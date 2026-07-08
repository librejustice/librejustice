-- Migration 0001 — Schéma initial LibreJustice (Postgres + ParadeDB + VectorChord)
--
-- Schéma minimal : seuls les champs dérivables de manière fiable sans
-- parser XML sont stockés sur ``decisions`` (source_uid + juridiction_type
-- issus de la provenance fichier). Les champs structurés (dates, type_recours,
-- solution…) seront ajoutés après la réécriture du from_xml.

CREATE EXTENSION IF NOT EXISTS pg_search;
CREATE EXTENSION IF NOT EXISTS vchord CASCADE;
-- vchord CASCADE installe pgvector si ce n'est pas déjà fait.

-- =====================================================================
-- 1. Métadonnées décisionnelles (minimal fiable)
-- =====================================================================

CREATE TABLE decisions (
    id               BIGSERIAL PRIMARY KEY,
    source_uid       TEXT        NOT NULL UNIQUE,   -- ex. TA_202208.zip/TA34/DTA_2204150_20220829.xml
    juridiction_type TEXT        NOT NULL,           -- TA | CAA | CE (dérivé du préfixe source_uid)
    content_hash     TEXT        NOT NULL,           -- sha256(xml brut) → idempotence ingest
    deleted_at       TIMESTAMPTZ,                   -- soft-delete (documents_reverses)
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_decisions_jur ON decisions (juridiction_type);

-- =====================================================================
-- 2. Chunks de recherche (BM25 + ANN)
-- =====================================================================

CREATE TABLE decision_chunks (
    id               BIGSERIAL PRIMARY KEY,
    decision_id      BIGINT      NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    chunk_index      INTEGER     NOT NULL,
    juridiction_type TEXT        NOT NULL,   -- dénormalisé de decisions pour filtre BM25 Tantivy
    char_start       INTEGER     NOT NULL,
    char_end         INTEGER     NOT NULL,
    body             TEXT        NOT NULL,   -- own_i = x[q_i:q_{i+1}], BM25-indexable
    embedding        vector(1024),           -- 1024 dim Qwen3-Embedding-0.6B, NULL avant embed
    UNIQUE (decision_id, chunk_index)
);

CREATE INDEX idx_decision_chunks_decision_id ON decision_chunks (decision_id);

-- BM25 ParadeDB : body + filtre Tantivy juridiction_type.
CREATE INDEX chunks_bm25 ON decision_chunks
USING bm25 (id, body, juridiction_type)
WITH (
  key_field = 'id',
  text_fields = '{"body": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "position"}}'
);

-- ANN VectorChord (RaBitQ 8-bit, distance cosinus).
CREATE INDEX chunks_vec ON decision_chunks
USING vchordrq (embedding vector_cosine_ops);

-- =====================================================================
-- 3. Texte intégral compressé (table froide)
-- =====================================================================

CREATE TABLE decision_full_text (
    decision_id  BIGINT  PRIMARY KEY REFERENCES decisions(id) ON DELETE CASCADE,
    gzip_blob    BYTEA   NOT NULL,
    raw_bytes    INTEGER NOT NULL,
    gzip_bytes   INTEGER NOT NULL
);
