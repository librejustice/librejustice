-- 0001_init.sql
CREATE EXTENSION IF NOT EXISTS pg_search;
CREATE EXTENSION IF NOT EXISTS vchord CASCADE;

CREATE TABLE decisions (
    id               BIGSERIAL PRIMARY KEY,
    source_uid       TEXT        NOT NULL UNIQUE,
    juridiction_type TEXT        NOT NULL,
    content_hash     TEXT        NOT NULL,
    deleted_at       TIMESTAMPTZ,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_decisions_jur ON decisions (juridiction_type);

CREATE TABLE decision_chunks (
    id               BIGSERIAL PRIMARY KEY,
    decision_id      BIGINT      NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    chunk_index      INTEGER     NOT NULL,
    juridiction_type TEXT        NOT NULL,
    char_start       INTEGER     NOT NULL,
    char_end         INTEGER     NOT NULL,
    body             TEXT        NOT NULL,
    embedding        vector(1024),
    UNIQUE (decision_id, chunk_index)
);

CREATE INDEX idx_decision_chunks_decision_id ON decision_chunks (decision_id);

CREATE INDEX chunks_bm25 ON decision_chunks
USING bm25 (id, body, juridiction_type)
WITH (
  key_field = 'id',
  text_fields = '{"body": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "position"}}'
);

CREATE INDEX chunks_vec ON decision_chunks
USING vchordrq (embedding vector_cosine_ops);

CREATE TABLE decision_full_text (
    decision_id  BIGINT  PRIMARY KEY REFERENCES decisions(id) ON DELETE CASCADE,
    gzip_blob    BYTEA   NOT NULL,
    raw_bytes    INTEGER NOT NULL,
    gzip_bytes   INTEGER NOT NULL
);

-- 0002_drop_decision_articles.sql
DROP TABLE IF EXISTS decision_articles;

-- 0003_add_source_xml_gzip.sql
ALTER TABLE decision_full_text
ADD COLUMN IF NOT EXISTS source_xml_gzip BYTEA;

-- 0004_decisions_public_id.sql
ALTER TABLE decisions
ADD COLUMN IF NOT EXISTS public_id TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_decisions_public_id
ON decisions (public_id)
WHERE public_id IS NOT NULL;

-- 0005_drop_source_xml_size_columns.sql
ALTER TABLE decision_full_text
DROP COLUMN IF EXISTS source_xml_raw_bytes;

ALTER TABLE decision_full_text
DROP COLUMN IF EXISTS source_xml_gzip_bytes;

-- 0006_gzip_blob_nullable.sql
ALTER TABLE decision_full_text ALTER COLUMN gzip_blob  DROP NOT NULL;
ALTER TABLE decision_full_text ALTER COLUMN raw_bytes  DROP NOT NULL;
ALTER TABLE decision_full_text ALTER COLUMN gzip_bytes DROP NOT NULL;

-- 0007_drop_gzip_blob_columns.sql
ALTER TABLE decision_full_text DROP COLUMN IF EXISTS gzip_blob;
ALTER TABLE decision_full_text DROP COLUMN IF EXISTS raw_bytes;
ALTER TABLE decision_full_text DROP COLUMN IF EXISTS gzip_bytes;

-- 0008_rename_source_xml_gzip.sql
ALTER TABLE decision_full_text DROP COLUMN IF EXISTS gzip_blob;
ALTER TABLE decision_full_text DROP COLUMN IF EXISTS raw_bytes;
ALTER TABLE decision_full_text DROP COLUMN IF EXISTS gzip_bytes;
ALTER TABLE decision_full_text RENAME COLUMN source_xml_gzip TO xml_gzip;

-- 0009_decisions_extracted_fields.sql
ALTER TABLE decisions
  ADD COLUMN IF NOT EXISTS date_lecture         DATE,
  ADD COLUMN IF NOT EXISTS date_audience        DATE,
  ADD COLUMN IF NOT EXISTS docket_numbers       TEXT[],
  ADD COLUMN IF NOT EXISTS jurisdiction_level   TEXT,
  ADD COLUMN IF NOT EXISTS instance_level       TEXT,
  ADD COLUMN IF NOT EXISTS formation_or_chamber TEXT,
  ADD COLUMN IF NOT EXISTS publication_code     TEXT,
  ADD COLUMN IF NOT EXISTS main_outcome         TEXT,
  ADD COLUMN IF NOT EXISTS special_procedure    TEXT,
  ADD COLUMN IF NOT EXISTS legal_references     JSONB;

ALTER TABLE decisions
  ADD COLUMN IF NOT EXISTS is_recueil     BOOLEAN
    GENERATED ALWAYS AS (publication_code LIKE '%A%') STORED,
  ADD COLUMN IF NOT EXISTS is_tables_lebon BOOLEAN
    GENERATED ALWAYS AS (publication_code LIKE '%B%') STORED;

CREATE INDEX IF NOT EXISTS idx_decisions_date_lecture
  ON decisions (date_lecture) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_decisions_jurisdiction_level
  ON decisions (jurisdiction_level) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_decisions_instance_level
  ON decisions (instance_level) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_decisions_main_outcome
  ON decisions (main_outcome) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_decisions_special_procedure
  ON decisions (special_procedure) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_decisions_is_recueil
  ON decisions (is_recueil) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_decisions_docket_numbers
  ON decisions USING GIN (docket_numbers) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_decisions_legal_references
  ON decisions USING GIN (legal_references) WHERE deleted_at IS NULL;

-- 0010_mcp_oauth_tables.sql
CREATE TABLE mcp_clients (
    client_id   TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    redirect_uri TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE mcp_auth_codes (
    code                    TEXT PRIMARY KEY,
    user_id                 TEXT NOT NULL,
    client_id               TEXT NOT NULL REFERENCES mcp_clients (client_id) ON DELETE CASCADE,
    code_challenge          TEXT NOT NULL,
    code_challenge_method   TEXT NOT NULL DEFAULT 'S256',
    redirect_uri            TEXT NOT NULL,
    expires_at              TIMESTAMPTZ NOT NULL,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX ON mcp_auth_codes (expires_at);

CREATE TABLE mcp_tokens (
    access_token TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL,
    client_id    TEXT NOT NULL,
    expires_at   TIMESTAMPTZ NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX ON mcp_tokens (expires_at);

-- 0011_semantic_keywords_tables.sql
CREATE TABLE semantic_vocabulary (
    word      TEXT PRIMARY KEY,
    embedding vector(1024) NOT NULL
);

CREATE TABLE decision_semantic_keywords (
    decision_id BIGINT PRIMARY KEY REFERENCES decisions(id) ON DELETE CASCADE,
    keywords    TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    expressions TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 0012_semantic_keyword_scores.sql
ALTER TABLE decision_semantic_keywords
ADD COLUMN keyword_scores REAL[] NOT NULL DEFAULT ARRAY[]::REAL[];

ALTER TABLE decision_semantic_keywords
ADD COLUMN expression_scores REAL[] NOT NULL DEFAULT ARRAY[]::REAL[];

-- 0013_decisions_jurisdiction_name.sql
ALTER TABLE decisions
  ADD COLUMN IF NOT EXISTS jurisdiction_name TEXT;

-- 0014_corpus_token_df_tables.sql
CREATE TABLE corpus_token_df (
    token TEXT    PRIMARY KEY,
    df    INTEGER NOT NULL
);

CREATE TABLE corpus_stats (
    key   TEXT   PRIMARY KEY,
    value BIGINT NOT NULL
);

-- 0015_decisions_search_title.sql
ALTER TABLE decisions ADD COLUMN IF NOT EXISTS search_title TEXT GENERATED ALWAYS AS (
  COALESCE(jurisdiction_name, juridiction_type)
  || CASE WHEN formation_or_chamber IS NOT NULL THEN ', ' || formation_or_chamber ELSE '' END
  || CASE WHEN date_lecture IS NOT NULL THEN
       ', ' ||
       EXTRACT(DAY FROM date_lecture)::int::text || ' ' ||
       (ARRAY['janvier','février','mars','avril','mai','juin','juillet',
              'août','septembre','octobre','novembre','décembre'])[EXTRACT(MONTH FROM date_lecture)::int] ||
       ' ' || EXTRACT(YEAR FROM date_lecture)::int::text
     ELSE '' END
  || CASE WHEN docket_numbers[1] IS NOT NULL THEN ', ' || docket_numbers[1] ELSE '' END
) STORED;

CREATE INDEX IF NOT EXISTS decisions_title_bm25 ON decisions
USING bm25 (id, search_title)
WITH (
  key_field = 'id',
  text_fields = '{"search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "position"}}'
);

-- 0016_search_title_bm25_stopwords.sql
DROP INDEX IF EXISTS decisions_title_bm25;

CREATE INDEX decisions_title_bm25 ON decisions
USING bm25 (id, (search_title::pdb.simple('stopwords_language=French', 'ascii_folding=true')))
WITH (key_field = 'id');

-- 0017_drop_unused_tables.sql
DROP TABLE IF EXISTS decision_semantic_keywords;
DROP TABLE IF EXISTS semantic_vocabulary;
DROP TABLE IF EXISTS corpus_token_df;
DROP TABLE IF EXISTS corpus_stats;
DROP TABLE IF EXISTS mcp_clients CASCADE;

-- 0018_decisions_content_checksum.sql
ALTER TABLE decisions ADD COLUMN content_checksum text;

-- 0019_full_text_payload_and_format.sql
ALTER TABLE decision_full_text RENAME COLUMN xml_gzip TO source_payload_gzip;

ALTER TABLE decision_full_text
  ADD COLUMN payload_format TEXT NOT NULL DEFAULT 'xml'
    CHECK (payload_format IN ('xml', 'json'));

ALTER TABLE decision_full_text ALTER COLUMN payload_format DROP DEFAULT;

-- 0020_drop_chunks_search_title.sql
DROP TRIGGER IF EXISTS trg_decisions_propagate_search_title ON decisions;
DROP TRIGGER IF EXISTS trg_chunks_inherit_search_title ON decision_chunks;
DROP FUNCTION IF EXISTS _sync_chunk_search_title_from_decision();
DROP FUNCTION IF EXISTS _set_chunk_search_title();

ALTER TABLE decision_chunks DROP COLUMN IF EXISTS search_title;

DROP INDEX IF EXISTS chunks_bm25;
CREATE INDEX chunks_bm25 ON decision_chunks
USING bm25 (id, body, juridiction_type)
WITH (
  key_field = 'id',
  text_fields = '{"body": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "position"}}'
);

-- 0021_search_title_bm25_regex_stopwords.sql
DROP INDEX IF EXISTS decisions_title_bm25;

CREATE INDEX decisions_title_bm25 ON decisions
USING bm25 (id, search_title)
WITH (
  key_field = 'id',
  text_fields = '{"search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "stopwords_language": "French", "record": "position"}}'
);

-- 0022_search_title_bm25_tokenizer_fix.sql
DROP INDEX IF EXISTS decisions_title_bm25;

CREATE INDEX decisions_title_bm25 ON decisions
USING bm25 (id, search_title)
WITH (
  key_field = 'id',
  text_fields = '{"search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French"}, "record": "position"}}'
);

-- 0023_chunks_embedding_rabitq8.sql
DROP INDEX IF EXISTS chunks_vec;

ALTER TABLE decision_chunks
    ALTER COLUMN embedding TYPE rabitq8(1024)
    USING quantize_to_rabitq8(embedding)::rabitq8(1024);

CREATE INDEX chunks_vec ON decision_chunks
USING vchordrq (embedding rabitq8_cosine_ops);

-- 0024_drop_lab_tables.sql
DROP TABLE IF EXISTS lab_keyword_metrics;
DROP TABLE IF EXISTS lab_vocab;

-- 0025_search_title_bm25_stopword_a.sql
DROP INDEX IF EXISTS decisions_title_bm25;

CREATE INDEX decisions_title_bm25 ON decisions
USING bm25 (id, search_title)
WITH (
  key_field = 'id',
  text_fields = '{"search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}}'
);

-- 0026_decision_chunks_denorm.sql
SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

DELETE FROM decisions WHERE deleted_at IS NOT NULL;

DROP INDEX IF EXISTS idx_decisions_date_lecture;
DROP INDEX IF EXISTS idx_decisions_jurisdiction_level;
DROP INDEX IF EXISTS idx_decisions_instance_level;
DROP INDEX IF EXISTS idx_decisions_main_outcome;
DROP INDEX IF EXISTS idx_decisions_special_procedure;
DROP INDEX IF EXISTS idx_decisions_is_recueil;
DROP INDEX IF EXISTS idx_decisions_docket_numbers;
DROP INDEX IF EXISTS idx_decisions_legal_references;

ALTER TABLE decisions DROP COLUMN deleted_at;

CREATE INDEX idx_decisions_date_lecture        ON decisions (date_lecture);
CREATE INDEX idx_decisions_jurisdiction_level  ON decisions (jurisdiction_level);
CREATE INDEX idx_decisions_instance_level      ON decisions (instance_level);
CREATE INDEX idx_decisions_main_outcome        ON decisions (main_outcome);
CREATE INDEX idx_decisions_special_procedure   ON decisions (special_procedure);
CREATE INDEX idx_decisions_is_recueil          ON decisions (is_recueil);
CREATE INDEX idx_decisions_docket_numbers      ON decisions USING GIN (docket_numbers);

CREATE TABLE decision_legal_references (
    decision_id BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    instrument  TEXT   NOT NULL,
    article     TEXT
);

CREATE INDEX idx_dlr_decision   ON decision_legal_references (decision_id);
CREATE INDEX idx_dlr_article    ON decision_legal_references (article text_pattern_ops)
  WHERE article IS NOT NULL;

CREATE INDEX idx_dlr_instrument ON decision_legal_references USING hash (instrument);

WITH src AS (
  SELECT
    d.id AS decision_id,
    CASE
      WHEN length(lr->>'instrument') > 200
        THEN split_part(lr->>'instrument', '. ', 1)
      ELSE lr->>'instrument'
    END AS instrument,
    lr->'articles' AS articles
  FROM decisions d,
       jsonb_array_elements(COALESCE(d.legal_references, '[]'::jsonb)) AS lr
  WHERE lr->>'instrument' IS NOT NULL
)
INSERT INTO decision_legal_references (decision_id, instrument, article)
SELECT
    src.decision_id,
    src.instrument,
    art_text
FROM src
LEFT JOIN LATERAL jsonb_array_elements_text(
    CASE
      WHEN jsonb_typeof(src.articles) = 'array'
        AND jsonb_array_length(src.articles) > 0
      THEN src.articles
      ELSE '[null]'::jsonb
    END
) AS art_text ON TRUE
WHERE length(src.instrument) BETWEEN 1 AND 500;

ALTER TABLE decisions DROP COLUMN legal_references;

DROP INDEX IF EXISTS chunks_vec;
DROP INDEX IF EXISTS chunks_bm25;

ALTER TABLE decision_chunks
    ADD COLUMN jurisdiction_level TEXT,
    ADD COLUMN instance_level     TEXT,
    ADD COLUMN main_outcome       TEXT,
    ADD COLUMN special_procedure  TEXT,
    ADD COLUMN jurisdiction_name  TEXT,
    ADD COLUMN is_recueil         BOOLEAN,
    ADD COLUMN is_tables_lebon    BOOLEAN,
    ADD COLUMN date_lecture       DATE,
    ADD COLUMN legal_instruments  TEXT[];

UPDATE decision_chunks c SET
    jurisdiction_level = d.jurisdiction_level,
    instance_level     = d.instance_level,
    main_outcome       = d.main_outcome,
    special_procedure  = d.special_procedure,
    jurisdiction_name  = d.jurisdiction_name,
    is_recueil         = d.is_recueil,
    is_tables_lebon    = d.is_tables_lebon,
    date_lecture       = d.date_lecture,
    legal_instruments  = li.instruments
FROM decisions d
LEFT JOIN LATERAL (
    SELECT ARRAY_AGG(DISTINCT instrument ORDER BY instrument) AS instruments
    FROM decision_legal_references
    WHERE decision_id = d.id
) li ON TRUE
WHERE c.decision_id = d.id;

CREATE INDEX idx_chunks_jurisdiction_level ON decision_chunks (jurisdiction_level);
CREATE INDEX idx_chunks_instance_level     ON decision_chunks (instance_level);
CREATE INDEX idx_chunks_main_outcome       ON decision_chunks (main_outcome);
CREATE INDEX idx_chunks_special_procedure  ON decision_chunks (special_procedure);
CREATE INDEX idx_chunks_jurisdiction_name  ON decision_chunks (LOWER(jurisdiction_name));
CREATE INDEX idx_chunks_is_recueil         ON decision_chunks (is_recueil);
CREATE INDEX idx_chunks_is_tables_lebon    ON decision_chunks (is_tables_lebon);
CREATE INDEX idx_chunks_date_lecture       ON decision_chunks (date_lecture);
CREATE INDEX idx_chunks_legal_instruments  ON decision_chunks USING GIN (legal_instruments);

CREATE OR REPLACE FUNCTION sync_chunks_decision_meta() RETURNS TRIGGER AS $$
BEGIN
    UPDATE decision_chunks SET
        jurisdiction_level = NEW.jurisdiction_level,
        instance_level     = NEW.instance_level,
        main_outcome       = NEW.main_outcome,
        special_procedure  = NEW.special_procedure,
        jurisdiction_name  = NEW.jurisdiction_name,
        is_recueil         = NEW.is_recueil,
        is_tables_lebon    = NEW.is_tables_lebon,
        date_lecture       = NEW.date_lecture
    WHERE decision_id = NEW.id;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER decisions_sync_chunks_meta
    AFTER UPDATE ON decisions
    FOR EACH ROW
    WHEN (
        OLD.jurisdiction_level IS DISTINCT FROM NEW.jurisdiction_level
     OR OLD.instance_level     IS DISTINCT FROM NEW.instance_level
     OR OLD.main_outcome       IS DISTINCT FROM NEW.main_outcome
     OR OLD.special_procedure  IS DISTINCT FROM NEW.special_procedure
     OR OLD.jurisdiction_name  IS DISTINCT FROM NEW.jurisdiction_name
     OR OLD.is_recueil         IS DISTINCT FROM NEW.is_recueil
     OR OLD.is_tables_lebon    IS DISTINCT FROM NEW.is_tables_lebon
     OR OLD.date_lecture       IS DISTINCT FROM NEW.date_lecture
    )
    EXECUTE FUNCTION sync_chunks_decision_meta();

CREATE OR REPLACE FUNCTION sync_chunks_legal_instruments() RETURNS TRIGGER AS $$
DECLARE
    target_decision_id BIGINT;
BEGIN
    target_decision_id := COALESCE(NEW.decision_id, OLD.decision_id);
    UPDATE decision_chunks SET legal_instruments = (
        SELECT ARRAY_AGG(DISTINCT instrument ORDER BY instrument)
        FROM decision_legal_references
        WHERE decision_id = target_decision_id
    )
    WHERE decision_id = target_decision_id;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER dlr_sync_chunks_legal_instruments
    AFTER INSERT OR UPDATE OR DELETE ON decision_legal_references
    FOR EACH ROW
    EXECUTE FUNCTION sync_chunks_legal_instruments();

CREATE INDEX chunks_bm25 ON decision_chunks
USING bm25 (
    id,
    body,
    (juridiction_type::pdb.literal),
    (jurisdiction_level::pdb.literal),
    (instance_level::pdb.literal),
    (main_outcome::pdb.literal),
    (special_procedure::pdb.literal),
    (jurisdiction_name::pdb.literal_normalized),
    (legal_instruments::pdb.literal),
    is_recueil,
    is_tables_lebon,
    date_lecture
)
WITH (
  key_field = 'id',
  text_fields = '{"body": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "position"}}'
);

CREATE INDEX chunks_vec ON decision_chunks
USING vchordrq (embedding rabitq8_cosine_ops);

-- 0027_legal_codes_3nf.sql
SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

CREATE TABLE legal_codes (
    id   BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    UNIQUE (name)
);

CREATE TABLE legal_articles (
    id      BIGSERIAL PRIMARY KEY,
    code_id BIGINT NOT NULL REFERENCES legal_codes(id) ON DELETE CASCADE,
    label   TEXT,
    UNIQUE NULLS NOT DISTINCT (code_id, label)
);
CREATE INDEX idx_legal_articles_code ON legal_articles(code_id);

INSERT INTO legal_codes (name)
SELECT DISTINCT instrument
FROM decision_legal_references;

INSERT INTO legal_articles (code_id, label)
SELECT DISTINCT lc.id, dlr.article
FROM decision_legal_references dlr
JOIN legal_codes lc ON lc.name = dlr.instrument;

CREATE TABLE decision_legal_references_v2 (
    decision_id BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    article_id  BIGINT NOT NULL REFERENCES legal_articles(id),
    PRIMARY KEY (decision_id, article_id)
);

INSERT INTO decision_legal_references_v2 (decision_id, article_id)
SELECT DISTINCT dlr.decision_id, la.id
FROM decision_legal_references dlr
JOIN legal_codes    lc ON lc.name    = dlr.instrument
JOIN legal_articles la ON la.code_id = lc.id
                       AND la.label  = dlr.article;

DROP TABLE decision_legal_references CASCADE;
ALTER TABLE decision_legal_references_v2 RENAME TO decision_legal_references;
ALTER INDEX decision_legal_references_v2_pkey RENAME TO decision_legal_references_pkey;

CREATE INDEX idx_dlr_article_id ON decision_legal_references(article_id);

CREATE OR REPLACE FUNCTION sync_chunks_legal_instruments() RETURNS TRIGGER AS $$
DECLARE
    target_decision_id BIGINT;
BEGIN
    target_decision_id := COALESCE(NEW.decision_id, OLD.decision_id);
    UPDATE decision_chunks SET legal_instruments = (
        SELECT ARRAY_AGG(DISTINCT lc.name ORDER BY lc.name)
        FROM decision_legal_references dlr
        JOIN legal_articles la ON la.id = dlr.article_id
        JOIN legal_codes    lc ON lc.id = la.code_id
        WHERE dlr.decision_id = target_decision_id
    )
    WHERE decision_id = target_decision_id;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER dlr_sync_chunks_legal_instruments
    AFTER INSERT OR UPDATE OR DELETE ON decision_legal_references
    FOR EACH ROW
    EXECUTE FUNCTION sync_chunks_legal_instruments();

-- 0028_dlr_trigger_guard.sql
CREATE OR REPLACE FUNCTION sync_chunks_legal_instruments() RETURNS TRIGGER AS $$
DECLARE
    target_decision_id BIGINT;
    new_arr TEXT[];
BEGIN
    target_decision_id := COALESCE(NEW.decision_id, OLD.decision_id);
    SELECT ARRAY_AGG(DISTINCT lc.name ORDER BY lc.name) INTO new_arr
    FROM decision_legal_references dlr
    JOIN legal_articles la ON la.id = dlr.article_id
    JOIN legal_codes    lc ON lc.id = la.code_id
    WHERE dlr.decision_id = target_decision_id;

    UPDATE decision_chunks
    SET legal_instruments = new_arr
    WHERE decision_id = target_decision_id
      AND legal_instruments IS DISTINCT FROM new_arr;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

-- 0029_dlr_trigger_statement_level.sql
DROP TRIGGER IF EXISTS dlr_sync_chunks_legal_instruments ON decision_legal_references;

CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT t.id AS decision_id,
               ARRAY_AGG(DISTINCT lc.name ORDER BY lc.name)
                   FILTER (WHERE lc.name IS NOT NULL) AS arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN decision_legal_references dlr ON dlr.decision_id = t.id
        LEFT JOIN legal_articles la             ON la.id = dlr.article_id
        LEFT JOIN legal_codes    lc             ON lc.id = la.code_id
        GROUP BY t.id
    )
    UPDATE decision_chunks c
    SET legal_instruments = agg.arr
    FROM agg
    WHERE c.decision_id = agg.decision_id
      AND c.legal_instruments IS DISTINCT FROM agg.arr;
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION sync_chunks_legal_instruments_ins() RETURNS TRIGGER AS $$
BEGIN
    PERFORM _sync_chunks_legal_instruments_for(
        ARRAY(SELECT DISTINCT decision_id FROM new_rows)
    );
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION sync_chunks_legal_instruments_del() RETURNS TRIGGER AS $$
BEGIN
    PERFORM _sync_chunks_legal_instruments_for(
        ARRAY(SELECT DISTINCT decision_id FROM old_rows)
    );
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION sync_chunks_legal_instruments_upd() RETURNS TRIGGER AS $$
BEGIN
    PERFORM _sync_chunks_legal_instruments_for(
        ARRAY(
            SELECT decision_id FROM new_rows
            UNION
            SELECT decision_id FROM old_rows
        )
    );
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER dlr_sync_chunks_legal_instruments_ins
    AFTER INSERT ON decision_legal_references
    REFERENCING NEW TABLE AS new_rows
    FOR EACH STATEMENT
    EXECUTE FUNCTION sync_chunks_legal_instruments_ins();

CREATE TRIGGER dlr_sync_chunks_legal_instruments_del
    AFTER DELETE ON decision_legal_references
    REFERENCING OLD TABLE AS old_rows
    FOR EACH STATEMENT
    EXECUTE FUNCTION sync_chunks_legal_instruments_del();

CREATE TRIGGER dlr_sync_chunks_legal_instruments_upd
    AFTER UPDATE ON decision_legal_references
    REFERENCING NEW TABLE AS new_rows OLD TABLE AS old_rows
    FOR EACH STATEMENT
    EXECUTE FUNCTION sync_chunks_legal_instruments_upd();

DROP FUNCTION IF EXISTS sync_chunks_legal_instruments();

-- 0030_drop_docket_numbers_gin.sql
DROP INDEX IF EXISTS idx_decisions_docket_numbers;

-- 0031_payload_gzip_storage_external.sql
ALTER TABLE decision_full_text
    ALTER COLUMN source_payload_gzip SET STORAGE EXTERNAL;

-- 0032_drop_decisions_content_hash.sql
ALTER TABLE decisions ALTER COLUMN content_checksum SET NOT NULL;
ALTER TABLE decisions DROP COLUMN content_hash;

-- 0033_chunks_vec_ivf_lists.sql
DROP INDEX chunks_vec;

CREATE INDEX chunks_vec ON decision_chunks
USING vchordrq (embedding rabitq8_cosine_ops)
WITH (options = $$
[build.internal]
lists = [2000]
build_threads = 8
spherical_centroids = true
$$);

ANALYZE decision_chunks;

-- 0034_users_table.sql
CREATE TABLE users (
    sub           TEXT PRIMARY KEY,
    email         TEXT,
    display_name  TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_users_last_seen_at ON users (last_seen_at);

-- 0035_user_bookmarks_table.sql
CREATE TABLE user_bookmarks (
    user_sub     TEXT   NOT NULL REFERENCES users (sub) ON DELETE CASCADE,
    decision_id  BIGINT NOT NULL REFERENCES decisions (id) ON DELETE CASCADE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_sub, decision_id)
);

CREATE INDEX idx_user_bookmarks_user_created
    ON user_bookmarks (user_sub, created_at DESC);

-- 0036_user_search_history_table.sql
CREATE TABLE user_search_history (
    id          BIGSERIAL PRIMARY KEY,
    user_sub    TEXT  NOT NULL REFERENCES users (sub) ON DELETE CASCADE,
    query       TEXT  NOT NULL,
    filters     JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_user_search_history_user_created
    ON user_search_history (user_sub, created_at DESC);

-- 0037_mcp_clients_redirect_uris.sql
DROP TABLE IF EXISTS mcp_clients CASCADE;

CREATE TABLE mcp_clients (
    client_id     TEXT PRIMARY KEY,
    name          TEXT,
    redirect_uris TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 0038_chunks_legal_article_labels.sql
SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

ALTER TABLE decision_chunks
    ADD COLUMN IF NOT EXISTS legal_article_labels TEXT[];

CREATE INDEX IF NOT EXISTS idx_chunks_legal_article_labels
    ON decision_chunks USING GIN (legal_article_labels);

UPDATE decision_chunks c
SET legal_article_labels = labels_agg.labels
FROM (
    SELECT
        dlr.decision_id,
        ARRAY_AGG(DISTINCT la.label ORDER BY la.label)
            FILTER (WHERE la.label IS NOT NULL) AS labels
    FROM decision_legal_references dlr
    JOIN legal_articles la ON la.id = dlr.article_id
    GROUP BY dlr.decision_id
) labels_agg
WHERE c.decision_id = labels_agg.decision_id
  AND c.legal_article_labels IS DISTINCT FROM labels_agg.labels;

CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT lc.name ORDER BY lc.name)
                FILTER (WHERE lc.name IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT la.label ORDER BY la.label)
                FILTER (WHERE la.label IS NOT NULL) AS labels_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN decision_legal_references dlr ON dlr.decision_id = t.id
        LEFT JOIN legal_articles la             ON la.id = dlr.article_id
        LEFT JOIN legal_codes    lc             ON lc.id = la.code_id
        GROUP BY t.id
    )
    UPDATE decision_chunks c
    SET legal_instruments     = agg.instruments_arr,
        legal_article_labels  = agg.labels_arr
    FROM agg
    WHERE c.decision_id = agg.decision_id
      AND (
            c.legal_instruments    IS DISTINCT FROM agg.instruments_arr
         OR c.legal_article_labels IS DISTINCT FROM agg.labels_arr
      );
$$ LANGUAGE sql;

DROP INDEX IF EXISTS chunks_bm25;

CREATE INDEX chunks_bm25 ON decision_chunks
USING bm25 (
    id,
    body,
    (juridiction_type::pdb.literal),
    (jurisdiction_level::pdb.literal),
    (instance_level::pdb.literal),
    (main_outcome::pdb.literal),
    (special_procedure::pdb.literal),
    (jurisdiction_name::pdb.literal_normalized),
    (legal_instruments::pdb.literal),
    (legal_article_labels::pdb.literal),
    is_recueil,
    is_tables_lebon,
    date_lecture
)
WITH (
  key_field = 'id',
  text_fields = '{"body": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "position"}}'
);

-- 0039_decisions_summary.sql
ALTER TABLE decisions
    ADD COLUMN summary TEXT,
    ADD COLUMN summary_prompt_version SMALLINT;

-- 0040_search_source_decision_views.sql
ALTER TABLE user_search_history
    ADD COLUMN source TEXT NOT NULL DEFAULT 'web';

CREATE TABLE user_decision_views (
    user_sub        TEXT    NOT NULL REFERENCES users (sub) ON DELETE CASCADE,
    decision_id     BIGINT  NOT NULL REFERENCES decisions (id) ON DELETE CASCADE,
    view_count      INTEGER NOT NULL DEFAULT 1,
    last_source     TEXT    NOT NULL DEFAULT 'web',
    first_viewed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_viewed_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_sub, decision_id)
);

CREATE INDEX idx_user_decision_views_user_viewed
    ON user_decision_views (user_sub, last_viewed_at DESC);

-- 0041_publication_codes_array.sql
SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

ALTER TABLE decisions
    ADD COLUMN publication_codes TEXT[] NOT NULL DEFAULT '{}';

UPDATE decisions
SET publication_codes = ARRAY[publication_code]
WHERE publication_code IS NOT NULL AND publication_code <> '';

DROP TRIGGER IF EXISTS decisions_sync_chunks_meta ON decisions;

ALTER TABLE decisions
    DROP COLUMN is_recueil,
    DROP COLUMN is_tables_lebon,
    DROP COLUMN publication_code;

UPDATE decisions SET docket_numbers = '{}' WHERE docket_numbers IS NULL;
ALTER TABLE decisions
    ALTER COLUMN docket_numbers SET DEFAULT '{}',
    ALTER COLUMN docket_numbers SET NOT NULL;

ALTER TABLE decision_chunks
    ADD COLUMN publication_codes TEXT[] NOT NULL DEFAULT '{}';

UPDATE decision_chunks c
SET publication_codes = d.publication_codes
FROM decisions d
WHERE c.decision_id = d.id
  AND c.publication_codes IS DISTINCT FROM d.publication_codes;

ALTER TABLE decision_chunks
    DROP COLUMN is_recueil,
    DROP COLUMN is_tables_lebon;

CREATE OR REPLACE FUNCTION sync_chunks_decision_meta() RETURNS TRIGGER AS $$
BEGIN
    UPDATE decision_chunks SET
        jurisdiction_level = NEW.jurisdiction_level,
        instance_level     = NEW.instance_level,
        main_outcome       = NEW.main_outcome,
        special_procedure  = NEW.special_procedure,
        jurisdiction_name  = NEW.jurisdiction_name,
        publication_codes  = NEW.publication_codes,
        date_lecture       = NEW.date_lecture
    WHERE decision_id = NEW.id;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER decisions_sync_chunks_meta
    AFTER UPDATE ON decisions
    FOR EACH ROW
    WHEN (
        OLD.jurisdiction_level IS DISTINCT FROM NEW.jurisdiction_level
     OR OLD.instance_level     IS DISTINCT FROM NEW.instance_level
     OR OLD.main_outcome       IS DISTINCT FROM NEW.main_outcome
     OR OLD.special_procedure  IS DISTINCT FROM NEW.special_procedure
     OR OLD.jurisdiction_name  IS DISTINCT FROM NEW.jurisdiction_name
     OR OLD.publication_codes  IS DISTINCT FROM NEW.publication_codes
     OR OLD.date_lecture       IS DISTINCT FROM NEW.date_lecture
    )
    EXECUTE FUNCTION sync_chunks_decision_meta();

DROP INDEX IF EXISTS chunks_bm25;

CREATE INDEX chunks_bm25 ON decision_chunks
USING bm25 (
    id,
    body,
    (juridiction_type::pdb.literal),
    (jurisdiction_level::pdb.literal),
    (instance_level::pdb.literal),
    (main_outcome::pdb.literal),
    (special_procedure::pdb.literal),
    (jurisdiction_name::pdb.literal_normalized),
    (legal_instruments::pdb.literal),
    (legal_article_labels::pdb.literal),
    (publication_codes::pdb.literal),
    date_lecture
)
WITH (
  key_field = 'id',
  text_fields = '{"body": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "position"}}'
);

-- 0042_users_track_activity.sql
ALTER TABLE users
    ADD COLUMN track_activity BOOLEAN NOT NULL DEFAULT true;

-- 0043_mcp_user_fk.sql
DELETE FROM mcp_tokens     t WHERE NOT EXISTS (SELECT 1 FROM users u WHERE u.sub = t.user_id);
DELETE FROM mcp_auth_codes c WHERE NOT EXISTS (SELECT 1 FROM users u WHERE u.sub = c.user_id);

ALTER TABLE mcp_tokens
    ADD CONSTRAINT mcp_tokens_user_fk
    FOREIGN KEY (user_id) REFERENCES users (sub) ON DELETE CASCADE;

ALTER TABLE mcp_auth_codes
    ADD CONSTRAINT mcp_auth_codes_user_fk
    FOREIGN KEY (user_id) REFERENCES users (sub) ON DELETE CASCADE;

-- 0044_mcp_client_fk.sql
DELETE FROM mcp_tokens     t WHERE NOT EXISTS (SELECT 1 FROM mcp_clients k WHERE k.client_id = t.client_id);
DELETE FROM mcp_auth_codes c WHERE NOT EXISTS (SELECT 1 FROM mcp_clients k WHERE k.client_id = c.client_id);

ALTER TABLE mcp_tokens
    ADD CONSTRAINT mcp_tokens_client_fk
    FOREIGN KEY (client_id) REFERENCES mcp_clients (client_id) ON DELETE CASCADE;

ALTER TABLE mcp_auth_codes
    ADD CONSTRAINT mcp_auth_codes_client_fk
    FOREIGN KEY (client_id) REFERENCES mcp_clients (client_id) ON DELETE CASCADE;

-- 0045_sitemaps_table.sql
CREATE TABLE IF NOT EXISTS sitemaps (
    filename     text PRIMARY KEY,
    content_type text NOT NULL,
    body         bytea NOT NULL,
    lastmod      date NOT NULL,
    updated_at   timestamptz NOT NULL DEFAULT now()
);

-- 0046_jurisdiction_name_literal.sql
SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

DROP INDEX IF EXISTS chunks_bm25;

CREATE INDEX chunks_bm25 ON decision_chunks
USING bm25 (
    id,
    body,
    (juridiction_type::pdb.literal),
    (jurisdiction_level::pdb.literal),
    (instance_level::pdb.literal),
    (main_outcome::pdb.literal),
    (special_procedure::pdb.literal),
    (jurisdiction_name::pdb.literal),
    (legal_instruments::pdb.literal),
    (legal_article_labels::pdb.literal),
    (publication_codes::pdb.literal),
    date_lecture
)
WITH (
  key_field = 'id',
  text_fields = '{"body": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "position"}}'
);

-- 0047_chunks_legal_article_composite.sql
SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

ALTER TABLE decision_chunks
    ADD COLUMN IF NOT EXISTS legal_article_composite TEXT[];

CREATE INDEX IF NOT EXISTS idx_chunks_legal_article_composite
    ON decision_chunks USING GIN (legal_article_composite);

UPDATE decision_chunks c
SET legal_article_composite = comp_agg.composite
FROM (
    SELECT
        dlr.decision_id,
        ARRAY_AGG(DISTINCT lc.name || '|' || la.label
                  ORDER BY lc.name || '|' || la.label)
            FILTER (WHERE la.label IS NOT NULL) AS composite
    FROM decision_legal_references dlr
    JOIN legal_articles la ON la.id = dlr.article_id
    JOIN legal_codes    lc ON lc.id = la.code_id
    GROUP BY dlr.decision_id
) comp_agg
WHERE c.decision_id = comp_agg.decision_id
  AND c.legal_article_composite IS DISTINCT FROM comp_agg.composite;

CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT lc.name ORDER BY lc.name)
                FILTER (WHERE lc.name IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT la.label ORDER BY la.label)
                FILTER (WHERE la.label IS NOT NULL) AS labels_arr,
            ARRAY_AGG(DISTINCT lc.name || '|' || la.label
                      ORDER BY lc.name || '|' || la.label)
                FILTER (WHERE la.label IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN decision_legal_references dlr ON dlr.decision_id = t.id
        LEFT JOIN legal_articles la             ON la.id = dlr.article_id
        LEFT JOIN legal_codes    lc             ON lc.id = la.code_id
        GROUP BY t.id
    )
    UPDATE decision_chunks c
    SET legal_instruments        = agg.instruments_arr,
        legal_article_labels     = agg.labels_arr,
        legal_article_composite  = agg.composite_arr
    FROM agg
    WHERE c.decision_id = agg.decision_id
      AND (
            c.legal_instruments       IS DISTINCT FROM agg.instruments_arr
         OR c.legal_article_labels    IS DISTINCT FROM agg.labels_arr
         OR c.legal_article_composite IS DISTINCT FROM agg.composite_arr
      );
$$ LANGUAGE sql;

-- 0048_chunks_bm25_unique.sql
SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

ALTER TABLE decision_chunks ADD COLUMN IF NOT EXISTS search_title TEXT;

CREATE OR REPLACE FUNCTION sync_chunks_decision_meta()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    UPDATE decision_chunks SET
        jurisdiction_level = NEW.jurisdiction_level,
        instance_level     = NEW.instance_level,
        main_outcome       = NEW.main_outcome,
        special_procedure  = NEW.special_procedure,
        jurisdiction_name  = NEW.jurisdiction_name,
        publication_codes  = NEW.publication_codes,
        date_lecture       = NEW.date_lecture,
        search_title       = CASE WHEN chunk_index = 0 THEN NEW.search_title END
    WHERE decision_id = NEW.id;
    RETURN NULL;
END;
$$;

DROP TRIGGER IF EXISTS decisions_sync_chunks_meta ON decisions;
CREATE TRIGGER decisions_sync_chunks_meta
AFTER UPDATE ON decisions
FOR EACH ROW
WHEN (
       OLD.jurisdiction_level IS DISTINCT FROM NEW.jurisdiction_level
    OR OLD.instance_level     IS DISTINCT FROM NEW.instance_level
    OR OLD.main_outcome       IS DISTINCT FROM NEW.main_outcome
    OR OLD.special_procedure  IS DISTINCT FROM NEW.special_procedure
    OR OLD.jurisdiction_name  IS DISTINCT FROM NEW.jurisdiction_name
    OR OLD.publication_codes  IS DISTINCT FROM NEW.publication_codes
    OR OLD.date_lecture       IS DISTINCT FROM NEW.date_lecture
    OR OLD.search_title       IS DISTINCT FROM NEW.search_title
)
EXECUTE FUNCTION sync_chunks_decision_meta();

CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT lc.name ORDER BY lc.name)
                FILTER (WHERE lc.name IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT lc.name || '|' || la.label
                      ORDER BY lc.name || '|' || la.label)
                FILTER (WHERE la.label IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN decision_legal_references dlr ON dlr.decision_id = t.id
        LEFT JOIN legal_articles la             ON la.id = dlr.article_id
        LEFT JOIN legal_codes    lc             ON lc.id = la.code_id
        GROUP BY t.id
    )
    UPDATE decision_chunks c
    SET legal_instruments       = agg.instruments_arr,
        legal_article_composite = agg.composite_arr
    FROM agg
    WHERE c.decision_id = agg.decision_id
      AND (
            c.legal_instruments       IS DISTINCT FROM agg.instruments_arr
         OR c.legal_article_composite IS DISTINCT FROM agg.composite_arr
      );
$$ LANGUAGE sql;

DROP INDEX IF EXISTS chunks_bm25;
DROP INDEX IF EXISTS decisions_title_bm25;
DROP INDEX IF EXISTS idx_chunks_legal_article_labels;
ALTER TABLE decision_chunks DROP COLUMN IF EXISTS legal_article_labels;

UPDATE decision_chunks c
SET search_title = d.search_title
FROM decisions d
WHERE d.id = c.decision_id
  AND c.chunk_index = 0
  AND c.search_title IS DISTINCT FROM d.search_title;

CREATE INDEX chunks_bm25 ON decision_chunks
USING bm25 (
    id,
    body,
    search_title,
    (juridiction_type::pdb.literal),
    (jurisdiction_level::pdb.literal),
    (instance_level::pdb.literal),
    (main_outcome::pdb.literal),
    (special_procedure::pdb.literal),
    (jurisdiction_name::pdb.literal),
    (legal_instruments::pdb.literal),
    (legal_article_composite::pdb.literal),
    (publication_codes::pdb.literal),
    date_lecture
)
WITH (
  key_field = 'id',
  text_fields = '{
    "body":         {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}
  }'
);

ANALYZE decision_chunks;

-- 0049_legal_codes_canonical.sql
ALTER TABLE legal_codes
    ADD COLUMN IF NOT EXISTS canonical_id BIGINT REFERENCES legal_codes(id);

ALTER TABLE legal_codes
    ADD CONSTRAINT legal_codes_canonical_not_self CHECK (canonical_id <> id);

CREATE INDEX IF NOT EXISTS idx_legal_codes_canonical
    ON legal_codes (canonical_id) WHERE canonical_id IS NOT NULL;

CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT COALESCE(canon.name, lc.name)
                      ORDER BY COALESCE(canon.name, lc.name))
                FILTER (WHERE lc.name IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT COALESCE(canon.name, lc.name) || '|' || la.label
                      ORDER BY COALESCE(canon.name, lc.name) || '|' || la.label)
                FILTER (WHERE la.label IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN decision_legal_references dlr ON dlr.decision_id = t.id
        LEFT JOIN legal_articles la             ON la.id = dlr.article_id
        LEFT JOIN legal_codes    lc             ON lc.id = la.code_id
        LEFT JOIN legal_codes    canon          ON canon.id = lc.canonical_id
        GROUP BY t.id
    )
    UPDATE decision_chunks c
    SET legal_instruments       = agg.instruments_arr,
        legal_article_composite = agg.composite_arr
    FROM agg
    WHERE c.decision_id = agg.decision_id
      AND (
            c.legal_instruments       IS DISTINCT FROM agg.instruments_arr
         OR c.legal_article_composite IS DISTINCT FROM agg.composite_arr
      );
$$ LANGUAGE sql;

-- 0050_decisions_extract_version.sql
ALTER TABLE decisions
    ADD COLUMN IF NOT EXISTS extract_version smallint;

-- 0051_full_text_source_fields_embed_ver.sql
ALTER TABLE decisions
    ADD COLUMN IF NOT EXISTS full_text     text,
    ADD COLUMN IF NOT EXISTS source_fields jsonb,
    ADD COLUMN IF NOT EXISTS embed_version smallint;

ALTER TABLE decisions
    ALTER COLUMN full_text SET COMPRESSION pglz;

-- 0052_decisions_legal_arrays.sql
ALTER TABLE decisions
    ADD COLUMN IF NOT EXISTS legal_instruments       TEXT[],
    ADD COLUMN IF NOT EXISTS legal_article_composite TEXT[];

CREATE OR REPLACE FUNCTION _sync_decisions_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT COALESCE(canon.name, lc.name)
                      ORDER BY COALESCE(canon.name, lc.name))
                FILTER (WHERE lc.name IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT COALESCE(canon.name, lc.name) || '|' || la.label
                      ORDER BY COALESCE(canon.name, lc.name) || '|' || la.label)
                FILTER (WHERE la.label IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN decision_legal_references dlr ON dlr.decision_id = t.id
        LEFT JOIN legal_articles la             ON la.id = dlr.article_id
        LEFT JOIN legal_codes    lc             ON lc.id = la.code_id
        LEFT JOIN legal_codes    canon          ON canon.id = lc.canonical_id
        GROUP BY t.id
    )
    UPDATE decisions d
    SET legal_instruments       = agg.instruments_arr,
        legal_article_composite = agg.composite_arr
    FROM agg
    WHERE d.id = agg.decision_id
      AND (
            d.legal_instruments       IS DISTINCT FROM agg.instruments_arr
         OR d.legal_article_composite IS DISTINCT FROM agg.composite_arr
      );
$$ LANGUAGE sql;

DO $$
DECLARE
    b     bigint := 0;
    maxid bigint;
    step  bigint := 20000;
BEGIN
    SELECT max(id) INTO maxid FROM decisions;
    WHILE maxid IS NOT NULL AND b < maxid LOOP
        PERFORM _sync_decisions_legal_instruments_for(
            ARRAY(SELECT id FROM decisions WHERE id > b AND id <= b + step)
        );
        b := b + step;
    END LOOP;
END $$;

ANALYZE decisions;

-- 0053_decisions_bm25.sql
SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

CREATE INDEX decisions_bm25 ON decisions
USING bm25 (
    id,
    full_text,
    search_title,
    (juridiction_type::pdb.literal),
    (jurisdiction_level::pdb.literal),
    (instance_level::pdb.literal),
    (main_outcome::pdb.literal),
    (special_procedure::pdb.literal),
    (jurisdiction_name::pdb.literal),
    (legal_instruments::pdb.literal),
    (legal_article_composite::pdb.literal),
    (publication_codes::pdb.literal),
    date_lecture
)
WITH (
  key_field = 'id',
  text_fields = '{
    "full_text":    {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}
  }'
);

ANALYZE decisions;

-- 0054_drop_chunks_bm25_body_payload.sql
DROP INDEX IF EXISTS chunks_bm25;

ALTER TABLE decision_chunks DROP COLUMN IF EXISTS body;

ALTER TABLE decision_full_text DROP COLUMN IF EXISTS source_payload_gzip;

-- 0055_decision_links.sql
CREATE TABLE decision_links (
    src_decision_id BIGINT   NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    dst_decision_id BIGINT   NULL     REFERENCES decisions(id) ON DELETE SET NULL,
    dst_key         TEXT     NULL,
    kind            SMALLINT NOT NULL,

    CONSTRAINT decision_links_target_present
        CHECK (dst_decision_id IS NOT NULL OR dst_key IS NOT NULL)
);

ALTER TABLE decision_links
    ADD CONSTRAINT decision_links_pk
    UNIQUE NULLS NOT DISTINCT (src_decision_id, kind, dst_decision_id, dst_key);

CREATE INDEX idx_decision_links_src ON decision_links (src_decision_id, kind);

CREATE INDEX idx_decision_links_dst ON decision_links (dst_decision_id, kind)
    WHERE dst_decision_id IS NOT NULL;

CREATE INDEX idx_decision_links_dst_key ON decision_links (dst_key)
    WHERE dst_decision_id IS NULL;

ALTER TABLE decisions
    ADD COLUMN IF NOT EXISTS links_version smallint;

-- 0056_decision_sources_and_ecli.sql
ALTER TABLE decisions
ADD COLUMN IF NOT EXISTS ecli TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_decisions_ecli
ON decisions (ecli)
WHERE ecli IS NOT NULL;

CREATE TABLE decision_sources (
    id               BIGSERIAL PRIMARY KEY,
    decision_id      BIGINT   NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    source           TEXT     NOT NULL,
    source_uid       TEXT     NOT NULL UNIQUE,
    content_checksum TEXT     NOT NULL,
    source_rank      SMALLINT NOT NULL,
    payload_format   TEXT     NOT NULL CHECK (payload_format IN ('xml','json','dila-xml','html')),
    ingested_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_decision_sources_decision_id ON decision_sources (decision_id);

-- 0057_legi_codes_and_articles.sql
CREATE TABLE legi_codes (
    legitext              TEXT PRIMARY KEY,
    code_court            TEXT,
    titre                 TEXT NOT NULL,
    nature                TEXT NOT NULL,
    derniere_modification DATE
);

CREATE INDEX idx_legi_codes_code_court ON legi_codes (code_court)
    WHERE code_court IS NOT NULL;

CREATE TABLE legi_articles (
    id               BIGSERIAL PRIMARY KEY,
    legiarti         TEXT NOT NULL UNIQUE,
    legitext         TEXT NOT NULL,
    num              TEXT NOT NULL,
    num_key          TEXT NOT NULL,
    titre_text       TEXT,
    etat             TEXT NOT NULL,
    date_debut       DATE NOT NULL,
    date_fin         DATE,
    texte            TEXT,
    nota             TEXT,
    content_checksum BIGINT NOT NULL
);

CREATE INDEX idx_legi_articles_at_date ON legi_articles (legitext, num_key, date_debut);

CREATE INDEX idx_legi_articles_vigueur ON legi_articles (legitext, num_key)
    WHERE etat = 'VIGUEUR';

ALTER TABLE legal_codes ADD COLUMN legitext TEXT REFERENCES legi_codes(legitext);

-- 0058_full_text_payload_format_html.sql
ALTER TABLE decision_full_text
  DROP CONSTRAINT decision_full_text_payload_format_check;

ALTER TABLE decision_full_text
  ADD CONSTRAINT decision_full_text_payload_format_check
    CHECK (payload_format IN ('xml', 'json', 'dila-xml', 'html'));

-- 0059_full_text_payload_format_pdf.sql
ALTER TABLE decision_full_text
  DROP CONSTRAINT decision_full_text_payload_format_check;

ALTER TABLE decision_full_text
  ADD CONSTRAINT decision_full_text_payload_format_check
    CHECK (payload_format IN ('xml', 'json', 'dila-xml', 'html', 'pdf'));

-- 0060_referential_multi_source.sql
ALTER TABLE legal_codes DROP COLUMN IF EXISTS legitext;
DROP TABLE IF EXISTS legi_articles;
DROP TABLE IF EXISTS legi_codes;

CREATE TABLE referential_texts (
    id            BIGSERIAL PRIMARY KEY,
    source        TEXT NOT NULL,
    source_uid    TEXT NOT NULL,
    jurisdiction  TEXT NOT NULL,
    slug          TEXT,
    title         TEXT NOT NULL,
    title_key     TEXT NOT NULL,
    nature        TEXT NOT NULL,
    last_modified DATE,
    UNIQUE (source, source_uid)
);

CREATE INDEX idx_referential_texts_slug ON referential_texts (slug) WHERE slug IS NOT NULL;
CREATE INDEX idx_referential_texts_title_key ON referential_texts (title_key);

CREATE INDEX referential_texts_title_bm25 ON referential_texts
USING bm25 (id, (title::pdb.simple('stopwords_language=French', 'ascii_folding=true')))
WITH (key_field = 'id');

CREATE TABLE referential_articles (
    id               BIGSERIAL PRIMARY KEY,
    source           TEXT NOT NULL,
    source_uid       TEXT NOT NULL,
    text_source_uid  TEXT NOT NULL,
    num              TEXT NOT NULL,
    num_key          TEXT NOT NULL,
    title_path       TEXT,
    status           TEXT NOT NULL,
    date_debut       DATE,
    date_fin         DATE,
    texte            TEXT,
    nota             TEXT,
    content_checksum BIGINT NOT NULL,
    UNIQUE (source, source_uid)
);

CREATE INDEX idx_ref_articles_at_date
    ON referential_articles (source, text_source_uid, num_key, date_debut);

CREATE INDEX idx_ref_articles_vigueur
    ON referential_articles (source, text_source_uid, num_key)
    WHERE status = 'VIGUEUR';

ALTER TABLE legal_codes ADD COLUMN referential_text_id BIGINT REFERENCES referential_texts(id);

ALTER TABLE legal_articles ADD COLUMN num_key TEXT;
CREATE INDEX idx_legal_articles_num_key ON legal_articles (code_id, num_key);

CREATE TABLE legal_article_resolution (
    article_id          BIGINT PRIMARY KEY REFERENCES legal_articles(id) ON DELETE CASCADE,
    ref_source          TEXT NOT NULL,
    ref_text_source_uid TEXT NOT NULL,
    num_key             TEXT NOT NULL
);

CREATE INDEX idx_lar_reverse
    ON legal_article_resolution (ref_source, ref_text_source_uid, num_key);

-- 0061_multisource_provenance_pivot.sql
ALTER TABLE decision_sources
    ADD COLUMN IF NOT EXISTS source_fields jsonb,
    ADD COLUMN IF NOT EXISTS deleted_at    timestamptz;

ALTER TABLE decision_sources
    ALTER COLUMN source_fields SET COMPRESSION pglz;

CREATE INDEX IF NOT EXISTS idx_decision_sources_active
    ON decision_sources (decision_id)
    WHERE deleted_at IS NULL;

ALTER TABLE decisions
    ADD COLUMN IF NOT EXISTS identity_key text,
    ADD COLUMN IF NOT EXISTS deleted_at   timestamptz;

CREATE INDEX IF NOT EXISTS idx_decisions_identity_key
    ON decisions (identity_key)
    WHERE identity_key IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_decisions_active
    ON decisions (id)
    WHERE deleted_at IS NULL;

-- 0062_drop_decision_links.sql
DROP TABLE IF EXISTS decision_links;

ALTER TABLE decisions DROP COLUMN IF EXISTS links_version;

-- 0063_drop_decisions_mono_source.sql
DO $$
DECLARE
    missing_prov   bigint;
    missing_fields bigint;
BEGIN
    SELECT count(*) INTO missing_prov
    FROM decisions d
    WHERE NOT EXISTS (
        SELECT 1 FROM decision_sources s WHERE s.source_uid = d.source_uid
    );

    SELECT count(*) INTO missing_fields
    FROM decisions d
    WHERE d.source_fields IS NOT NULL
      AND NOT EXISTS (
        SELECT 1 FROM decision_sources s
        WHERE s.source_uid = d.source_uid AND s.source_fields IS NOT NULL
      );

    IF missing_prov > 0 OR missing_fields > 0 THEN
        RAISE EXCEPTION
            'Portage incomplet — % décision(s) sans provenance, % source_fields non porté(s). Lancer `lj-ingest dedup-backfill` (ou `backfill-decision-sources`) AVANT le DROP.',
            missing_prov, missing_fields;
    END IF;
END $$;

ALTER TABLE decisions
    DROP COLUMN IF EXISTS source_uid,
    DROP COLUMN IF EXISTS content_checksum,
    DROP COLUMN IF EXISTS source_fields;

-- 0064_decision_sources_payload_format_pdf.sql
ALTER TABLE decision_sources
  DROP CONSTRAINT decision_sources_payload_format_check;

ALTER TABLE decision_sources
  ADD CONSTRAINT decision_sources_payload_format_check
    CHECK (payload_format IN ('xml', 'json', 'dila-xml', 'html', 'pdf'));

-- 0065_rename_identity_key_to_canonical_ref.sql
ALTER TABLE decisions
    RENAME COLUMN identity_key TO canonical_ref;

ALTER INDEX idx_decisions_identity_key
    RENAME TO idx_decisions_canonical_ref;

-- 0066_ambiguous_canonical_ref.sql
CREATE TABLE IF NOT EXISTS ambiguous_canonical_ref (
    canonical_ref TEXT PRIMARY KEY,
    n_distinct    INT,
    reason        TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 0067_canonical_ref_ambiguous_column.sql
ALTER TABLE decisions
    ADD COLUMN IF NOT EXISTS canonical_ref_ambiguous boolean NOT NULL DEFAULT false;

DROP TABLE IF EXISTS ambiguous_canonical_ref;

-- 0068_drop_canonical_ref_ambiguous.sql
ALTER TABLE decisions
    DROP COLUMN IF EXISTS canonical_ref_ambiguous;

-- 0069_ecli_non_unique.sql
DROP INDEX IF EXISTS idx_decisions_ecli;

CREATE INDEX IF NOT EXISTS idx_decisions_ecli
ON decisions (ecli)
WHERE ecli IS NOT NULL;

-- 0070_source_rank_generated.sql
ALTER TABLE decision_sources DROP COLUMN source_rank;

ALTER TABLE decision_sources
    ADD COLUMN source_rank SMALLINT NOT NULL GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN 60
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-capp'    THEN 30
            ELSE 0
        END
    ) STORED;

-- 0071_payload_format_docx.sql
ALTER TABLE decision_full_text
  DROP CONSTRAINT decision_full_text_payload_format_check;

ALTER TABLE decision_full_text
  ADD CONSTRAINT decision_full_text_payload_format_check
    CHECK (payload_format IN ('xml', 'json', 'dila-xml', 'html', 'pdf', 'docx'));

ALTER TABLE decision_sources
  DROP CONSTRAINT decision_sources_payload_format_check;

ALTER TABLE decision_sources
  ADD CONSTRAINT decision_sources_payload_format_check
    CHECK (payload_format IN ('xml', 'json', 'dila-xml', 'html', 'pdf', 'docx'));

-- 0072_legal_text_legal_article.sql
ALTER TABLE referential_texts RENAME TO legal_text;
ALTER TABLE legal_text RENAME COLUMN source_uid TO text_uid;
ALTER TABLE legal_text ADD COLUMN date_texte DATE;
ALTER TABLE legal_text ADD COLUMN date_publi DATE;

ALTER TABLE legal_text DROP CONSTRAINT referential_texts_source_source_uid_key;
ALTER TABLE legal_text DROP COLUMN source;
ALTER TABLE legal_text ADD CONSTRAINT legal_text_text_uid_key UNIQUE (text_uid);
ALTER INDEX idx_referential_texts_slug RENAME TO idx_legal_text_slug;
ALTER INDEX idx_referential_texts_title_key RENAME TO idx_legal_text_title_key;

ALTER TABLE referential_articles RENAME TO legal_article;
ALTER TABLE legal_article RENAME COLUMN text_source_uid TO text_uid;
ALTER TABLE legal_article ADD COLUMN position INTEGER;
ALTER TABLE legal_article ADD COLUMN source_url TEXT;

UPDATE legal_article SET date_debut = DATE '0001-01-01' WHERE date_debut IS NULL;

DELETE FROM legal_article a
USING (
    SELECT id, row_number() OVER (
        PARTITION BY text_uid, num_key, date_debut
        ORDER BY length(coalesce(texte, '')) DESC, id DESC
    ) AS rn
    FROM legal_article
) d
WHERE a.id = d.id AND d.rn > 1;

ALTER TABLE legal_article ALTER COLUMN date_debut SET NOT NULL;
ALTER TABLE legal_article DROP CONSTRAINT referential_articles_source_source_uid_key;
ALTER TABLE legal_article DROP CONSTRAINT referential_articles_pkey;
ALTER TABLE legal_article DROP COLUMN id;
ALTER TABLE legal_article ADD PRIMARY KEY (text_uid, num_key, date_debut);

DROP INDEX idx_ref_articles_at_date;
DROP INDEX idx_ref_articles_vigueur;
CREATE INDEX idx_legal_article_vigueur ON legal_article (text_uid, num_key) WHERE status = 'VIGUEUR';
CREATE INDEX idx_legal_article_reading ON legal_article (text_uid, position);

ALTER TABLE legal_article_resolution RENAME COLUMN ref_text_source_uid TO ref_text_uid;
ALTER TABLE legal_article_resolution DROP COLUMN ref_source;

CREATE INDEX idx_lar_reverse ON legal_article_resolution (ref_text_uid, num_key);

-- 0073_cited_reference_decision_citation.sql
CREATE TABLE cited_reference (
    id           BIGSERIAL PRIMARY KEY,
    text_key     TEXT NOT NULL,
    article_key  TEXT,
    raw_text     TEXT NOT NULL,
    raw_article  TEXT,
    ref_text_uid TEXT,
    ref_num_key  TEXT,

    UNIQUE NULLS NOT DISTINCT (text_key, article_key)
);

CREATE INDEX idx_cited_reference_text_key ON cited_reference (text_key);

CREATE INDEX idx_cited_reference_resolved
    ON cited_reference (ref_text_uid, ref_num_key)
    WHERE ref_text_uid IS NOT NULL;

CREATE TABLE decision_citation (
    decision_id        BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    cited_reference_id BIGINT NOT NULL REFERENCES cited_reference(id) ON DELETE CASCADE,
    PRIMARY KEY (decision_id, cited_reference_id)
);

CREATE INDEX idx_decision_citation_ref ON decision_citation (cited_reference_id);

-- 0074_legal_text_title_key_trgm.sql
CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE INDEX idx_legal_text_title_key_trgm
    ON legal_text USING gin (title_key gin_trgm_ops);

-- 0075_drop_title_key_trgm.sql
DROP INDEX IF EXISTS idx_legal_text_title_key_trgm;
DROP EXTENSION IF EXISTS pg_trgm;

-- 0076_legal_arrays_from_cited_reference.sql
CREATE OR REPLACE FUNCTION _sync_decisions_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT cr.text_key ORDER BY cr.text_key)
                FILTER (WHERE cr.text_key IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT cr.text_key || '|' || cr.article_key
                      ORDER BY cr.text_key || '|' || cr.article_key)
                FILTER (WHERE cr.article_key IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN decision_citation dc ON dc.decision_id = t.id
        LEFT JOIN cited_reference   cr ON cr.id = dc.cited_reference_id
        GROUP BY t.id
    )
    UPDATE decisions d
    SET legal_instruments       = agg.instruments_arr,
        legal_article_composite = agg.composite_arr
    FROM agg
    WHERE d.id = agg.decision_id
      AND (
            d.legal_instruments       IS DISTINCT FROM agg.instruments_arr
         OR d.legal_article_composite IS DISTINCT FROM agg.composite_arr
      );
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT cr.text_key ORDER BY cr.text_key)
                FILTER (WHERE cr.text_key IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT cr.text_key || '|' || cr.article_key
                      ORDER BY cr.text_key || '|' || cr.article_key)
                FILTER (WHERE cr.article_key IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN decision_citation dc ON dc.decision_id = t.id
        LEFT JOIN cited_reference   cr ON cr.id = dc.cited_reference_id
        GROUP BY t.id
    )
    UPDATE decision_chunks c
    SET legal_instruments       = agg.instruments_arr,
        legal_article_composite = agg.composite_arr
    FROM agg
    WHERE c.decision_id = agg.decision_id
      AND (
            c.legal_instruments       IS DISTINCT FROM agg.instruments_arr
         OR c.legal_article_composite IS DISTINCT FROM agg.composite_arr
      );
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION sync_citation_arrays_ins()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    ids bigint[] := ARRAY(SELECT DISTINCT decision_id FROM new_rows);
BEGIN
    PERFORM _sync_decisions_legal_instruments_for(ids);
    PERFORM _sync_chunks_legal_instruments_for(ids);
    RETURN NULL;
END;
$$;

CREATE OR REPLACE FUNCTION sync_citation_arrays_del()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    ids bigint[] := ARRAY(SELECT DISTINCT decision_id FROM old_rows);
BEGIN
    PERFORM _sync_decisions_legal_instruments_for(ids);
    PERFORM _sync_chunks_legal_instruments_for(ids);
    RETURN NULL;
END;
$$;

DROP TRIGGER IF EXISTS dlr_sync_chunks_legal_instruments_ins ON decision_legal_references;
DROP TRIGGER IF EXISTS dlr_sync_chunks_legal_instruments_del ON decision_legal_references;
DROP TRIGGER IF EXISTS dlr_sync_chunks_legal_instruments_upd ON decision_legal_references;

CREATE TRIGGER dc_sync_arrays_ins
    AFTER INSERT ON decision_citation
    REFERENCING NEW TABLE AS new_rows
    FOR EACH STATEMENT EXECUTE FUNCTION sync_citation_arrays_ins();

CREATE TRIGGER dc_sync_arrays_del
    AFTER DELETE ON decision_citation
    REFERENCING OLD TABLE AS old_rows
    FOR EACH STATEMENT EXECUTE FUNCTION sync_citation_arrays_del();

-- 0077_drop_legacy_3nf_citations.sql
DROP TABLE IF EXISTS decision_legal_references CASCADE;
DROP TABLE IF EXISTS legal_article_resolution CASCADE;
DROP TABLE IF EXISTS legal_articles CASCADE;
DROP TABLE IF EXISTS legal_codes CASCADE;

-- 0078_legal_article_bm25.sql
ALTER TABLE legal_article
    ADD COLUMN IF NOT EXISTS id BIGINT GENERATED BY DEFAULT AS IDENTITY;

CREATE UNIQUE INDEX IF NOT EXISTS legal_article_id_key ON legal_article (id);

SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

CREATE INDEX IF NOT EXISTS legal_article_bm25 ON legal_article
USING bm25 (
    id,
    texte,
    num,
    (source::pdb.literal),
    (text_uid::pdb.literal),
    (status::pdb.literal)
)
WITH (
  key_field = 'id',
  text_fields = '{
    "texte": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "num":   {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}
  }'
);

ANALYZE legal_article;

-- 0079_legal_article_search_title.sql
DROP INDEX IF EXISTS legal_article_bm25;

ALTER TABLE legal_article ADD COLUMN IF NOT EXISTS code_title TEXT;

ALTER TABLE legal_article ADD COLUMN IF NOT EXISTS search_title TEXT GENERATED ALWAYS AS (
  COALESCE(code_title, '')
  || ', ' || num
  || CASE WHEN title_path IS NOT NULL AND title_path <> ''
       THEN ', ' || regexp_replace(title_path, '^.*>\s*', '')
       ELSE '' END
) STORED;

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

-- 0080_legal_text_identity_cascade.sql
ALTER TABLE legal_text ADD COLUMN IF NOT EXISTS eli            TEXT;
ALTER TABLE legal_text ADD COLUMN IF NOT EXISTS nor            TEXT;
ALTER TABLE legal_text ADD COLUMN IF NOT EXISTS instrument_key TEXT;

CREATE INDEX IF NOT EXISTS idx_legal_text_eli ON legal_text (eli) WHERE eli IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_legal_text_nor ON legal_text (nor) WHERE nor IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_legal_text_instrument_key
    ON legal_text (instrument_key) WHERE instrument_key IS NOT NULL;

-- 0081_decisions_bm25_en_stopwords.sql
SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

DROP INDEX IF EXISTS decisions_bm25;

CREATE INDEX decisions_bm25 ON decisions
USING bm25 (
    id,
    full_text,
    search_title,
    (juridiction_type::pdb.literal),
    (jurisdiction_level::pdb.literal),
    (instance_level::pdb.literal),
    (main_outcome::pdb.literal),
    (special_procedure::pdb.literal),
    (jurisdiction_name::pdb.literal),
    (legal_instruments::pdb.literal),
    (legal_article_composite::pdb.literal),
    (publication_codes::pdb.literal),
    date_lecture
)
WITH (
  key_field = 'id',
  text_fields = '{
    "full_text":    {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"},
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}
  }'
);

ANALYZE decisions;

-- 0082_legal_article_lang_translation.sql
ALTER TABLE legal_article
    ADD COLUMN IF NOT EXISTS texte_original TEXT,
    ADD COLUMN IF NOT EXISTS lang_original  TEXT,
    ADD COLUMN IF NOT EXISTS translation    TEXT NOT NULL DEFAULT 'non_officiel'
        CHECK (translation IN ('officiel', 'non_officiel', 'automatique'));

UPDATE legal_article SET translation = 'officiel'
    WHERE source <> 'jafbase';
UPDATE legal_article SET translation = 'officiel'
    WHERE source = 'jafbase' AND nota ILIKE '%texte officiel%';

-- 0083_full_text_toast_lz4.sql
ALTER TABLE decisions ALTER COLUMN full_text SET COMPRESSION lz4;

-- 0084_lang_rank_authority.sql
CREATE FUNCTION lang_rank(fields jsonb) RETURNS int
    IMMUTABLE LANGUAGE sql AS $$
    SELECT CASE
        WHEN fields->>'languageisocode' = 'FRE'
          OR fields->>'resource_obtained_language' = 'fra'
        THEN 1 ELSE 0
    END
$$;

-- 0085_legal_article_source_asof.sql
ALTER TABLE legal_article
    ADD COLUMN IF NOT EXISTS source_asof         DATE,
    ADD COLUMN IF NOT EXISTS source_upstream_url TEXT;

CREATE TABLE IF NOT EXISTS ingest_freshness (
    source TEXT PRIMARY KEY,
    asof   DATE NOT NULL
);

INSERT INTO ingest_freshness (source, asof) VALUES
    ('legifrance', CURRENT_DATE),
    ('kali',       CURRENT_DATE),
    ('jorf',       CURRENT_DATE)
ON CONFLICT (source) DO NOTHING;

UPDATE legal_article SET source_asof = DATE '2025-10-04'
    WHERE source = 'jafbase' AND source_asof IS NULL;

-- 0086_treaty_jorf_get_date.sql
UPDATE legal_article SET source_asof = CURRENT_DATE
    WHERE source IN ('treaty', 'jorf') AND source_asof IS NULL;

DELETE FROM ingest_freshness WHERE source = 'jorf';

-- 0087_legal_text_num_prefix_agnostic.sql
ALTER TABLE legal_text
    ADD COLUMN IF NOT EXISTS num_prefix_agnostic BOOLEAN NOT NULL DEFAULT false;

-- 0088_legal_text_facet_indexes.sql
CREATE INDEX IF NOT EXISTS legal_text_jurisdiction_idx ON legal_text (jurisdiction);
CREATE INDEX IF NOT EXISTS legal_text_nature_idx ON legal_text (nature);

-- 0089_citation_override_certified_version.sql
CREATE TABLE citation_resolution_override (
    id           BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    text_key     TEXT NOT NULL,
    article_key  TEXT,
    ref_text_uid TEXT,
    ref_num_key  TEXT,
    version      INT  NOT NULL,
    reason       TEXT NOT NULL,
    source       TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_cro_key ON citation_resolution_override (lower(text_key), article_key);

ALTER TABLE decisions ADD COLUMN certified_version INT;

-- 0090_decision_citation_spans.sql
ALTER TABLE decision_citation ADD COLUMN char_starts INT[];
ALTER TABLE decision_citation ADD COLUMN char_ends   INT[];

-- 0091_legal_citation_direct_link.sql
CREATE TABLE legal_citation (
    id                    BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    decision_id           BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    char_start            INT,
    char_end              INT,
    surface_text          TEXT NOT NULL,
    text_key              TEXT,
    article_key           TEXT,
    ref_text_uid          TEXT,
    ref_num_key           TEXT,
    status                TEXT NOT NULL,
    non_citable_reason    TEXT,
    antecedent_char_start INT,
    antecedent_char_end   INT,
    source                TEXT NOT NULL,
    confidence            REAL,
    extract_version       SMALLINT,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_lc_decision ON legal_citation (decision_id, char_start);

CREATE INDEX idx_lc_ref ON legal_citation (ref_text_uid) WHERE ref_text_uid IS NOT NULL;

CREATE INDEX idx_lc_text_key ON legal_citation (lower(text_key));

-- 0092_gold_annotation.sql
CREATE TABLE gold_annotation (
    id                BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    decision_id       BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    version           INT  NOT NULL,
    kind              TEXT NOT NULL,
    payload           JSONB NOT NULL,
    source            TEXT NOT NULL,
    review_confidence REAL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (decision_id, kind, version)
);

CREATE INDEX idx_gold_decision ON gold_annotation (decision_id);

-- 0093_decision_extraction_columns.sql
ALTER TABLE decisions ADD COLUMN applicant_companies      TEXT[];
ALTER TABLE decisions ADD COLUMN applicant_individuals    TEXT[];
ALTER TABLE decisions ADD COLUMN applicant_counsel_names  TEXT[];
ALTER TABLE decisions ADD COLUMN applicant_law_firms      TEXT[];
ALTER TABLE decisions ADD COLUMN defendant_companies      TEXT[];
ALTER TABLE decisions ADD COLUMN defendant_individuals    TEXT[];
ALTER TABLE decisions ADD COLUMN defendant_counsel_names  TEXT[];
ALTER TABLE decisions ADD COLUMN defendant_law_firms      TEXT[];
ALTER TABLE decisions ADD COLUMN defendant_administration TEXT;
ALTER TABLE decisions ADD COLUMN intervenors             TEXT[];
ALTER TABLE decisions ADD COLUMN challenged_acts         TEXT[];

ALTER TABLE decisions ADD COLUMN decision_type              TEXT;
ALTER TABLE decisions ADD COLUMN detailed_main_outcome      TEXT;
ALTER TABLE decisions ADD COLUMN legal_domain               TEXT;
ALTER TABLE decisions ADD COLUMN jurisdiction_location_code  TEXT;
ALTER TABLE decisions ADD COLUMN jurisdiction_location_label TEXT;
ALTER TABLE decisions ADD COLUMN publication_scope          TEXT;
ALTER TABLE decisions ADD COLUMN publication_status         TEXT;
ALTER TABLE decisions ADD COLUMN search_keywords            TEXT[];
ALTER TABLE decisions ADD COLUMN lower_decisions            JSONB;

DROP TABLE IF EXISTS gold_annotation;

-- 0094_trim_extraction_columns.sql
ALTER TABLE decisions DROP COLUMN applicant_individuals;
ALTER TABLE decisions DROP COLUMN defendant_individuals;
ALTER TABLE decisions DROP COLUMN intervenors;
ALTER TABLE decisions DROP COLUMN challenged_acts;
ALTER TABLE decisions DROP COLUMN decision_type;
ALTER TABLE decisions DROP COLUMN detailed_main_outcome;
ALTER TABLE decisions DROP COLUMN jurisdiction_location_code;
ALTER TABLE decisions DROP COLUMN jurisdiction_location_label;
ALTER TABLE decisions DROP COLUMN publication_scope;
ALTER TABLE decisions DROP COLUMN publication_status;
ALTER TABLE decisions DROP COLUMN search_keywords;
ALTER TABLE decisions DROP COLUMN lower_decisions;

-- 0095_drop_legal_citation_source.sql
ALTER TABLE legal_citation DROP COLUMN source;

-- 0096_citation_model.sql
CREATE TABLE citation_key (
    id             int4 GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    text_key       text NOT NULL,
    article_key    text,

    nature         int2 NOT NULL,

    jurisdiction   text,
    act_date       date,
    act_num        text,
    citability     int2 NOT NULL DEFAULT 0,
    signal_version int2 NOT NULL,

    ref_text_uid   text REFERENCES legal_text(text_uid),
    ref_num_key    text,
    link_rule      int2,

    n_decisions    int4 NOT NULL DEFAULT 0,
    UNIQUE NULLS NOT DISTINCT (text_key, article_key),
    CHECK (ref_num_key IS NULL OR ref_text_uid IS NOT NULL),
    CHECK (ref_text_uid IS NULL OR link_rule IS NOT NULL)
);
CREATE INDEX idx_ck_lower_text ON citation_key (lower(text_key));
CREATE INDEX idx_ck_ref ON citation_key (ref_text_uid, ref_num_key)
    WHERE ref_text_uid IS NOT NULL;
CREATE INDEX idx_ck_pending ON citation_key (id)
    WHERE link_rule IS NULL AND citability = 0;

DROP TRIGGER IF EXISTS dc_sync_arrays_ins ON decision_citation;
DROP TRIGGER IF EXISTS dc_sync_arrays_del ON decision_citation;
DROP FUNCTION IF EXISTS sync_citation_arrays_ins();
DROP FUNCTION IF EXISTS sync_citation_arrays_del();

DROP TABLE legal_citation;

CREATE TABLE legal_citation (
    decision_id     int8 NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    char_start      int4 NOT NULL,
    char_end        int4 NOT NULL,
    key_id          int4 NOT NULL REFERENCES citation_key(id),
    extract_version int2 NOT NULL,
    PRIMARY KEY (decision_id, char_start),
    CHECK (char_end > char_start)
);
CREATE INDEX idx_lc_key ON legal_citation (key_id, decision_id);
ALTER TABLE legal_citation SET (fillfactor = 100,
    autovacuum_vacuum_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);

CREATE TABLE citation_occurrence_link (
    decision_id  int8 NOT NULL,
    char_start   int4 NOT NULL,
    status       int2 NOT NULL,

    ref_text_uid text REFERENCES legal_text(text_uid),
    ref_num_key  text,
    link_rule    int2 NOT NULL,
    antecedent_char_start int4,
    antecedent_char_end   int4,
    PRIMARY KEY (decision_id, char_start),
    FOREIGN KEY (decision_id, char_start)
      REFERENCES legal_citation (decision_id, char_start) ON DELETE CASCADE,
    CHECK ((status = 0) = (ref_text_uid IS NOT NULL)),
    CHECK (ref_num_key IS NULL OR ref_text_uid IS NOT NULL),
    CHECK ((antecedent_char_start IS NULL) = (antecedent_char_end IS NULL))
);
CREATE INDEX idx_col_ref ON citation_occurrence_link (ref_text_uid, ref_num_key)
    WHERE ref_text_uid IS NOT NULL;

CREATE TABLE citation_gold_anchor (
    decision_id        int8 PRIMARY KEY REFERENCES decisions(id) ON DELETE CASCADE,
    full_text_checksum int8 NOT NULL,
    loaded_at          timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE resolution_run (
    id          int4 GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    started_at  timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz,
    n_keys      int4 NOT NULL DEFAULT 0,
    n_gained    int4 NOT NULL DEFAULT 0,
    n_lost      int4 NOT NULL DEFAULT 0,
    n_moved     int4 NOT NULL DEFAULT 0,
    by_rule     jsonb
);

CREATE TABLE resolution_run_diff (
    run_id           int4 NOT NULL REFERENCES resolution_run(id) ON DELETE CASCADE,
    key_id           int4 NOT NULL,
    old_ref_text_uid text,
    new_ref_text_uid text,
    old_ref_num_key  text,
    new_ref_num_key  text,
    change           int2 NOT NULL,
    PRIMARY KEY (run_id, key_id)
);

-- 0097_citation_flat.sql
DROP TABLE resolution_run_diff;
DROP TABLE resolution_run;
DROP TABLE citation_occurrence_link;
DROP TABLE citation_gold_anchor;
DROP TABLE legal_citation;
DROP TABLE citation_key;

CREATE TABLE legal_citation (
    decision_id     int8 NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    char_start      int4 NOT NULL,
    char_end        int4 NOT NULL,
    ref_text_uid    text REFERENCES legal_text(text_uid),
    ref_num_key     text,
    extract_version int2 NOT NULL,
    PRIMARY KEY (decision_id, char_start),
    CHECK (char_end > char_start),
    CHECK (ref_num_key IS NULL OR ref_text_uid IS NOT NULL)
);
CREATE INDEX idx_lc_ref ON legal_citation (ref_text_uid, ref_num_key)
    WHERE ref_text_uid IS NOT NULL;
ALTER TABLE legal_citation SET (fillfactor = 100,
    autovacuum_vacuum_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);

-- 0098_facet_arrays_from_legal_citation.sql
CREATE OR REPLACE FUNCTION _sync_decisions_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT lc.ref_text_uid ORDER BY lc.ref_text_uid)
                FILTER (WHERE lc.ref_text_uid IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT lc.ref_text_uid || '|' || lc.ref_num_key
                      ORDER BY lc.ref_text_uid || '|' || lc.ref_num_key)
                FILTER (WHERE lc.ref_num_key IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN legal_citation lc ON lc.decision_id = t.id
        GROUP BY t.id
    )
    UPDATE decisions d
    SET legal_instruments       = agg.instruments_arr,
        legal_article_composite = agg.composite_arr
    FROM agg
    WHERE d.id = agg.decision_id
      AND (
            d.legal_instruments       IS DISTINCT FROM agg.instruments_arr
         OR d.legal_article_composite IS DISTINCT FROM agg.composite_arr
      );
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT lc.ref_text_uid ORDER BY lc.ref_text_uid)
                FILTER (WHERE lc.ref_text_uid IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT lc.ref_text_uid || '|' || lc.ref_num_key
                      ORDER BY lc.ref_text_uid || '|' || lc.ref_num_key)
                FILTER (WHERE lc.ref_num_key IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN legal_citation lc ON lc.decision_id = t.id
        GROUP BY t.id
    )
    UPDATE decision_chunks c
    SET legal_instruments       = agg.instruments_arr,
        legal_article_composite = agg.composite_arr
    FROM agg
    WHERE c.decision_id = agg.decision_id
      AND (
            c.legal_instruments       IS DISTINCT FROM agg.instruments_arr
         OR c.legal_article_composite IS DISTINCT FROM agg.composite_arr
      );
$$ LANGUAGE sql;

-- 0099_drop_legacy_citation_tables.sql
DROP TABLE IF EXISTS decision_citation;
DROP TABLE IF EXISTS citation_resolution_override;
DROP TABLE IF EXISTS cited_reference;

-- 0100_facet_value_jurisdiction_tables.sql
CREATE TABLE facet_value (
    uid        TEXT PRIMARY KEY,
    facet      TEXT NOT NULL,
    label      TEXT NOT NULL,
    abbr       TEXT,
    parent_uid TEXT REFERENCES facet_value(uid),
    sort       INT NOT NULL DEFAULT 0,
    CHECK (uid LIKE facet || ':%')
);

CREATE TABLE jurisdiction (
    code             TEXT PRIMARY KEY,
    juridiction_type TEXT NOT NULL,
    city             TEXT,
    label            TEXT NOT NULL
);

ALTER TABLE decisions
    ADD COLUMN legal_domain_uid  TEXT REFERENCES facet_value(uid)
                                 CHECK (legal_domain_uid LIKE 'domaine:%'),
    ADD COLUMN solution_uid      TEXT REFERENCES facet_value(uid)
                                 CHECK (solution_uid LIKE 'solution:%'),
    ADD COLUMN publication_uid   TEXT REFERENCES facet_value(uid)
                                 CHECK (publication_uid LIKE 'publication:%'),
    ADD COLUMN instance_uid      TEXT REFERENCES facet_value(uid)
                                 CHECK (instance_uid LIKE 'instance:%'),
    ADD COLUMN voie_uid          TEXT REFERENCES facet_value(uid)
                                 CHECK (voie_uid LIKE 'voie:%'),
    ADD COLUMN office_uid        TEXT REFERENCES facet_value(uid)
                                 CHECK (office_uid LIKE 'office:%'),
    ADD COLUMN jurisdiction_code TEXT REFERENCES jurisdiction(code);

INSERT INTO facet_value (uid, facet, label, parent_uid, sort) VALUES
    ('domaine:CIVIL',                    'domaine', 'Civil',                    NULL, 1),
    ('domaine:COMMERCIAL',               'domaine', 'Commercial',               NULL, 2),
    ('domaine:PUBLIC',                   'domaine', 'Public',                   NULL, 3),
    ('domaine:SOCIAL',                   'domaine', 'Social',                   NULL, 4),
    ('domaine:FISCAL',                   'domaine', 'Fiscal',                   NULL, 5),
    ('domaine:PROPRIETE_INTELLECTUELLE', 'domaine', 'Propriété intellectuelle', NULL, 6),
    ('domaine:EUROPEEN',                 'domaine', 'Européen',                 NULL, 7),
    ('domaine:CRIMINEL',                 'domaine', 'Criminel',                 NULL, 8),
    ('domaine:CONSTITUTIONNEL',          'domaine', 'Constitutionnel',          NULL, 9);

INSERT INTO facet_value (uid, facet, label, parent_uid, sort) VALUES
    ('domaine:CIVIL_PROCEDURES_CIVILES_EXECUTION',           'domaine', 'Procédures civiles d''exécution',                          'domaine:CIVIL', 1),
    ('domaine:CIVIL_DROIT_IMMOBILIER_CONSTRUCTION',          'domaine', 'Droit immobilier et de la construction',                   'domaine:CIVIL', 2),
    ('domaine:CIVIL_DROIT_LOCATIF',                          'domaine', 'Droit locatif',                                            'domaine:CIVIL', 3),
    ('domaine:CIVIL_DROIT_PERSONNES_FAMILLE',                'domaine', 'Droit des personnes et de la famille',                     'domaine:CIVIL', 4),
    ('domaine:CIVIL_DROIT_COPROPRIETE_PROPRIETE_IMMOBILIERE','domaine', 'Droit de la copropriété et de la propriété immobilière',   'domaine:CIVIL', 5),
    ('domaine:CIVIL_DROIT_ASSURANCES',                       'domaine', 'Droit des assurances',                                     'domaine:CIVIL', 6),
    ('domaine:CIVIL_DROIT_RESPONSABILITE',                   'domaine', 'Droit de la responsabilité',                               'domaine:CIVIL', 7),
    ('domaine:CIVIL_DROIT_BANCAIRE_BOURSIER',                'domaine', 'Droit bancaire et boursier',                               'domaine:CIVIL', 8),
    ('domaine:CIVIL_DROIT_SUCCESSIONS',                      'domaine', 'Droit des successions',                                    'domaine:CIVIL', 9),
    ('domaine:CIVIL_DROIT_EXPROPRIATION_PREEMPTION',         'domaine', 'Droit de l''expropriation et de préemption',               'domaine:CIVIL', 10),
    ('domaine:CIVIL_DIVORCE_SEPARATION_CORPS',               'domaine', 'Divorce et séparation de corps',                           'domaine:CIVIL', 11),
    ('domaine:CIVIL_DROIT_RURAL',                            'domaine', 'Droit rural',                                              'domaine:CIVIL', 12),
    ('domaine:CIVIL_DROIT_RESPONSABILITE_CONTRATS',          'domaine', 'Droit de la responsabilité et des contrats',               'domaine:CIVIL', 13),
    ('domaine:CIVIL_DROIT_SAISIE_IMMOBILIERE',               'domaine', 'Droit de la saisie immobilière',                           'domaine:CIVIL', 14),
    ('domaine:CIVIL_DROIT_MINEURS',                          'domaine', 'Droit des mineurs',                                        'domaine:CIVIL', 15),
    ('domaine:COMMERCIAL_DROIT_ENTREPRISES_DIFFICULTE',      'domaine', 'Droit des entreprises en difficulté',                      'domaine:COMMERCIAL', 1),
    ('domaine:COMMERCIAL_DROIT_BANCAIRE_BOURSIER',           'domaine', 'Droit bancaire et boursier',                               'domaine:COMMERCIAL', 2),
    ('domaine:COMMERCIAL_DROIT_CONTRATS',                    'domaine', 'Droit des contrats',                                       'domaine:COMMERCIAL', 3),
    ('domaine:COMMERCIAL_DROIT_SOCIETES',                    'domaine', 'Droit des sociétés',                                       'domaine:COMMERCIAL', 4),
    ('domaine:COMMERCIAL_DROIT_NUMERIQUE',                   'domaine', 'Droit du numérique',                                       'domaine:COMMERCIAL', 5),
    ('domaine:COMMERCIAL_DROIT_TRANSPORT',                   'domaine', 'Droit du transport',                                       'domaine:COMMERCIAL', 6),
    ('domaine:COMMERCIAL_DROIT_ASSURANCES',                  'domaine', 'Droit des assurances',                                     'domaine:COMMERCIAL', 7),
    ('domaine:COMMERCIAL_DROIT_CONCURRENCE',                 'domaine', 'Droit de la concurrence',                                  'domaine:COMMERCIAL', 8),
    ('domaine:COMMERCIAL_DROIT_CONSOMMATION',                'domaine', 'Droit de la consommation',                                 'domaine:COMMERCIAL', 9),
    ('domaine:COMMERCIAL_DROIT_ARBITRAGE',                   'domaine', 'Droit de l''arbitrage',                                    'domaine:COMMERCIAL', 10),
    ('domaine:PUBLIC_DROIT_ETRANGERS_NATIONALITE',           'domaine', 'Droit des étrangers et de la nationalité',                 'domaine:PUBLIC', 1),
    ('domaine:PUBLIC_DROIT_URBANISME_IMMOBILIER_PUBLIC',     'domaine', 'Droit de l''urbanisme et de l''immobilier public',         'domaine:PUBLIC', 2),
    ('domaine:PUBLIC_DROIT_TRAVAIL',                         'domaine', 'Droit public du travail',                                  'domaine:PUBLIC', 3),
    ('domaine:PUBLIC_DROIT_PENAL_PUBLIC',                    'domaine', 'Droit pénal public',                                       'domaine:PUBLIC', 4),
    ('domaine:PUBLIC_DROIT_AIDE_ACTION_SOCIALE',             'domaine', 'Droit de l''aide et de l''action sociale',                 'domaine:PUBLIC', 5),
    ('domaine:PUBLIC_DROIT_ENVIRONNEMENT',                   'domaine', 'Droit de l''environnement',                                'domaine:PUBLIC', 6),
    ('domaine:SOCIAL_DROIT_TRAVAIL',                         'domaine', 'Droit du travail',                                         'domaine:SOCIAL', 1),
    ('domaine:SOCIAL_DROIT_AIDE_ACTION_SOCIALE',             'domaine', 'Droit de l''aide et de l''action sociale',                 'domaine:SOCIAL', 2),
    ('domaine:SOCIAL_DROIT_PENAL_SOCIAL',                    'domaine', 'Droit pénal social',                                       'domaine:SOCIAL', 3),
    ('domaine:PROPRIETE_INTELLECTUELLE_INDUSTRIELLE',        'domaine', 'Propriété industrielle',                                   'domaine:PROPRIETE_INTELLECTUELLE', 1),
    ('domaine:PROPRIETE_INTELLECTUELLE_LITTERAIRE_ARTISTIQUE','domaine','Propriété littéraire et artistique',                       'domaine:PROPRIETE_INTELLECTUELLE', 2);

INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('solution:REJET',                  'solution', 'Rejet',                  1),
    ('solution:IRRECEVABILITE',         'solution', 'Irrecevabilité',         2),
    ('solution:DESISTEMENT',            'solution', 'Désistement',            3),
    ('solution:NON_LIEU_A_STATUER',     'solution', 'Non-lieu à statuer',     4),
    ('solution:CONFIRMATION',           'solution', 'Confirmation',           5),
    ('solution:INFIRMATION',            'solution', 'Infirmation',            6),
    ('solution:INFIRMATION_PARTIELLE',  'solution', 'Infirmation partielle',  7),
    ('solution:REFORMATION',            'solution', 'Réformation',            8),
    ('solution:CASSATION',              'solution', 'Cassation',              9),
    ('solution:CASSATION_PARTIELLE',    'solution', 'Cassation partielle',    10),
    ('solution:ANNULATION',             'solution', 'Annulation',             11),
    ('solution:CONFORMITE',             'solution', 'Conformité',             12),
    ('solution:NON_CONFORMITE',         'solution', 'Non conformité',         13),
    ('solution:INELIGIBILITE',          'solution', 'Inéligibilité',          14),
    ('solution:SATISFACTION_TOTALE',    'solution', 'Satisfaction totale',    15),
    ('solution:SATISFACTION_PARTIELLE', 'solution', 'Satisfaction partielle', 16),
    ('solution:AUTRE',                  'solution', 'Autre',                  17);

INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('publication:PUBLIE_BULLETIN',   'publication', 'Publié au bulletin',                     1),
    ('publication:INEDIT_BULLETIN',   'publication', 'Inédit au bulletin',                     2),
    ('publication:PUBLIE_LEBON',      'publication', 'Publié au recueil Lebon',                3),
    ('publication:MENTIONNE_LEBON',   'publication', 'Mentionné aux tables du recueil Lebon',  4),
    ('publication:INEDIT_LEBON',      'publication', 'Inédit au recueil Lebon',                5),
    ('publication:AUTRE',             'publication', 'Autre',                                  6);

INSERT INTO facet_value (uid, facet, label, abbr, sort) VALUES
    ('voie:REFERE_SUSPENSION',            'voie', 'Référé-suspension',                              NULL,   1),
    ('voie:REFERE_LIBERTE',               'voie', 'Référé-liberté',                                 NULL,   2),
    ('voie:REFERE_MESURES_UTILES',        'voie', 'Référé mesures utiles',                          NULL,   3),
    ('voie:REFERE_PRECONTRACTUEL',        'voie', 'Référé précontractuel',                          NULL,   4),
    ('voie:REFERE_PROVISION',             'voie', 'Référé-provision',                               NULL,   5),
    ('voie:REFERE_CIVIL',                 'voie', 'Référé civil',                                   NULL,   6),
    ('voie:FILTRAGE_R222_1',              'voie', 'Ordonnance de tri (R. 222-1 CJA)',               NULL,   7),
    ('voie:PAPC',                         'voie', 'Procédure d''admission des pourvois en cassation','PAPC', 8),
    ('voie:QPC',                          'voie', 'Question prioritaire de constitutionnalité',      'QPC',  9),
    ('voie:QUESTION_PREJUDICIELLE_CJUE',  'voie', 'Question préjudicielle à la CJUE',               NULL,   10),
    ('voie:RECOURS_REVISION',             'voie', 'Recours en révision',                            NULL,   11),
    ('voie:TIERCE_OPPOSITION',            'voie', 'Tierce opposition',                              NULL,   12),
    ('voie:RECTIFICATION_INTERPRETATION', 'voie', 'Rectification ou interprétation',                NULL,   13);

INSERT INTO facet_value (uid, facet, label, abbr, sort) VALUES
    ('office:JLD',               'office', 'Juge des libertés et de la détention',  'JLD', 1),
    ('office:JAF',               'office', 'Juge aux affaires familiales',          'JAF', 2),
    ('office:JCP',               'office', 'Juge des contentieux de la protection', 'JCP', 3),
    ('office:JEX',               'office', 'Juge de l''exécution',                  'JEX', 4),
    ('office:JUGE_ENFANTS',      'office', 'Juge des enfants',                      NULL,  5),
    ('office:PREMIER_PRESIDENT', 'office', 'Premier président',                     NULL,  6),
    ('office:MAGISTRAT_DESIGNE', 'office', 'Magistrat désigné',                     NULL,  7);

INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('instance:PREMIERE_INSTANCE',          'instance', 'Première instance',          1),
    ('instance:APPEL',                      'instance', 'Appel',                      2),
    ('instance:CASSATION',                  'instance', 'Cassation',                  3),
    ('instance:PREMIER_ET_DERNIER_RESSORT', 'instance', 'Premier et dernier ressort', 4),
    ('instance:RENVOI_APRES_CASSATION',     'instance', 'Renvoi après cassation',     5),
    ('instance:AUTRE',                      'instance', 'Autre',                      6);

-- 0101_facet_sync_functions_report_drift.sql
DROP FUNCTION _sync_decisions_legal_instruments_for(bigint[]);
DROP FUNCTION _sync_chunks_legal_instruments_for(bigint[]);

CREATE FUNCTION _sync_decisions_legal_instruments_for(p_ids bigint[])
RETURNS bigint AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT lc.ref_text_uid ORDER BY lc.ref_text_uid)
                FILTER (WHERE lc.ref_text_uid IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT lc.ref_text_uid || '|' || lc.ref_num_key
                      ORDER BY lc.ref_text_uid || '|' || lc.ref_num_key)
                FILTER (WHERE lc.ref_num_key IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN legal_citation lc ON lc.decision_id = t.id
        GROUP BY t.id
    ),
    upd AS (
        UPDATE decisions d
        SET legal_instruments       = agg.instruments_arr,
            legal_article_composite = agg.composite_arr
        FROM agg
        WHERE d.id = agg.decision_id
          AND (
                d.legal_instruments       IS DISTINCT FROM agg.instruments_arr
             OR d.legal_article_composite IS DISTINCT FROM agg.composite_arr
          )
        RETURNING 1
    )
    SELECT count(*) FROM upd;
$$ LANGUAGE sql;

CREATE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS bigint AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT lc.ref_text_uid ORDER BY lc.ref_text_uid)
                FILTER (WHERE lc.ref_text_uid IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT lc.ref_text_uid || '|' || lc.ref_num_key
                      ORDER BY lc.ref_text_uid || '|' || lc.ref_num_key)
                FILTER (WHERE lc.ref_num_key IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN legal_citation lc ON lc.decision_id = t.id
        GROUP BY t.id
    ),
    upd AS (
        UPDATE decision_chunks c
        SET legal_instruments       = agg.instruments_arr,
            legal_article_composite = agg.composite_arr
        FROM agg
        WHERE c.decision_id = agg.decision_id
          AND (
                c.legal_instruments       IS DISTINCT FROM agg.instruments_arr
             OR c.legal_article_composite IS DISTINCT FROM agg.composite_arr
          )
        RETURNING 1
    )
    SELECT count(*) FROM upd;
$$ LANGUAGE sql;

-- 0102_facet_juridiction_types.sql
INSERT INTO facet_value (uid, facet, label, abbr, sort) VALUES
    ('juridiction:CE',              'juridiction', 'Conseil d''État',                        'CE',           1),
    ('juridiction:CAA',             'juridiction', 'Cour administrative d''appel',           'CAA',          2),
    ('juridiction:TA',              'juridiction', 'Tribunal administratif',                 'TA',           3),
    ('juridiction:CC',              'juridiction', 'Cour de cassation',                      'CC',           4),
    ('juridiction:CA',              'juridiction', 'Cour d''appel',                          'CA',           5),
    ('juridiction:TJ',              'juridiction', 'Tribunal judiciaire',                    'TJ',           6),
    ('juridiction:TCOM',            'juridiction', 'Tribunal de commerce',                   'TCOM',         7),
    ('juridiction:CNDA',            'juridiction', 'Cour nationale du droit d''asile',       'CNDA',         8),
    ('juridiction:CONSTIT',         'juridiction', 'Conseil constitutionnel',                'Cons. const.', 9),
    ('juridiction:TC',              'juridiction', 'Tribunal des conflits',                  'TC',           10),
    ('juridiction:CEDH',            'juridiction', 'Cour européenne des droits de l''homme', 'CEDH',         11),
    ('juridiction:CJUE',            'juridiction', 'Cour de justice de l''Union européenne', 'CJUE',         12),
    ('juridiction:CE_ANALYSE',      'juridiction', 'Analyse (Conseil d''État)',              'Analyse CE',   13),
    ('juridiction:CE_CONCLUSIONS',  'juridiction', 'Conclusions du rapporteur public',       'Concl. RP',    14);

-- 0103_gold_annotation_revive.sql
CREATE TABLE gold_annotation (
    id                BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    decision_id       BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    version           INT  NOT NULL,
    kind              TEXT NOT NULL,
    payload           JSONB NOT NULL,
    source            TEXT NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (decision_id, kind, version)
);

CREATE INDEX idx_gold_decision ON gold_annotation (decision_id);

-- 0104_drop_gold_annotation.sql
DROP TABLE IF EXISTS gold_annotation;

-- 0105_add_served_lang_drop_lang_rank.sql
ALTER TABLE decision_sources ADD COLUMN served_lang text
    GENERATED ALWAYS AS (
        CASE
            WHEN source_fields->>'languageisocode' = 'FRE'
              OR source_fields->>'resource_obtained_language' = 'fra' THEN 'fra'
            WHEN source_fields->>'languageisocode' = 'ENG'
              OR source_fields->>'resource_obtained_language' = 'eng' THEN 'eng'
        END
    ) VIRTUAL;

DROP FUNCTION lang_rank(jsonb);

-- 0106_drop_served_lang_add_lang_column.sql
ALTER TABLE decision_sources DROP COLUMN served_lang;
ALTER TABLE decision_sources ADD COLUMN lang text;

UPDATE decision_sources SET lang = CASE
    WHEN source_fields->>'languageisocode' = 'FRE'
      OR source_fields->>'resource_obtained_language' = 'fra' THEN 'fra'
    WHEN source_fields->>'languageisocode' = 'ENG'
      OR source_fields->>'resource_obtained_language' = 'eng' THEN 'eng'
END
WHERE source IN ('cedh', 'cjue');

-- 0107_drop_legacy_facet_columns.sql
SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

CREATE OR REPLACE FUNCTION sync_chunks_decision_meta()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    UPDATE decision_chunks SET
        jurisdiction_name  = NEW.jurisdiction_name,
        publication_codes  = NEW.publication_codes,
        date_lecture       = NEW.date_lecture,
        search_title       = CASE WHEN chunk_index = 0 THEN NEW.search_title END
    WHERE decision_id = NEW.id;
    RETURN NULL;
END;
$$;

DROP TRIGGER IF EXISTS decisions_sync_chunks_meta ON decisions;
CREATE TRIGGER decisions_sync_chunks_meta
AFTER UPDATE ON decisions
FOR EACH ROW
WHEN (
    OLD.jurisdiction_name IS DISTINCT FROM NEW.jurisdiction_name
    OR OLD.publication_codes IS DISTINCT FROM NEW.publication_codes
    OR OLD.date_lecture IS DISTINCT FROM NEW.date_lecture
    OR OLD.search_title IS DISTINCT FROM NEW.search_title
)
EXECUTE FUNCTION sync_chunks_decision_meta();

DROP INDEX IF EXISTS idx_chunks_jurisdiction_level;
DROP INDEX IF EXISTS idx_chunks_instance_level;
DROP INDEX IF EXISTS idx_chunks_main_outcome;
DROP INDEX IF EXISTS idx_chunks_special_procedure;
ALTER TABLE decision_chunks
    DROP COLUMN IF EXISTS jurisdiction_level,
    DROP COLUMN IF EXISTS instance_level,
    DROP COLUMN IF EXISTS main_outcome,
    DROP COLUMN IF EXISTS special_procedure;

DROP INDEX IF EXISTS decisions_bm25;

DROP INDEX IF EXISTS idx_decisions_jurisdiction_level;
DROP INDEX IF EXISTS idx_decisions_instance_level;
DROP INDEX IF EXISTS idx_decisions_main_outcome;
DROP INDEX IF EXISTS idx_decisions_special_procedure;
ALTER TABLE decisions
    DROP COLUMN IF EXISTS jurisdiction_level,
    DROP COLUMN IF EXISTS instance_level,
    DROP COLUMN IF EXISTS main_outcome,
    DROP COLUMN IF EXISTS special_procedure,
    DROP COLUMN IF EXISTS legal_domain,
    DROP COLUMN IF EXISTS instance_uid,
    DROP COLUMN IF EXISTS defendant_administration;

DELETE FROM facet_value WHERE uid LIKE 'instance:%';

CREATE INDEX decisions_bm25 ON decisions
USING bm25 (
    id,
    full_text,
    search_title,
    (juridiction_type::pdb.literal),
    (jurisdiction_name::pdb.literal),
    (legal_instruments::pdb.literal),
    (legal_article_composite::pdb.literal),
    (publication_codes::pdb.literal),
    date_lecture
)
WITH (
  key_field = 'id',
  text_fields = '{
    "full_text":    {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"},
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}
  }'
);

ANALYZE decisions;
ANALYZE decision_chunks;

-- 0108_decision_themes.sql
ALTER TABLE decisions
    ADD COLUMN themes text[] NOT NULL DEFAULT '{}';

-- 0109_sync_arrays_collate_c.sql
CREATE OR REPLACE FUNCTION _sync_decisions_legal_instruments_for(p_ids bigint[])
RETURNS bigint
LANGUAGE sql
AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT lc.ref_text_uid COLLATE "C"
                      ORDER BY lc.ref_text_uid COLLATE "C")
                FILTER (WHERE lc.ref_text_uid IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT (lc.ref_text_uid || '|' || lc.ref_num_key) COLLATE "C"
                      ORDER BY (lc.ref_text_uid || '|' || lc.ref_num_key) COLLATE "C")
                FILTER (WHERE lc.ref_num_key IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN legal_citation lc ON lc.decision_id = t.id
        GROUP BY t.id
    ),
    upd AS (
        UPDATE decisions d
        SET legal_instruments       = agg.instruments_arr,
            legal_article_composite = agg.composite_arr
        FROM agg
        WHERE d.id = agg.decision_id
          AND (
                d.legal_instruments       IS DISTINCT FROM agg.instruments_arr
             OR d.legal_article_composite IS DISTINCT FROM agg.composite_arr
          )
        RETURNING 1
    )
    SELECT count(*) FROM upd;
$$;

CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS bigint
LANGUAGE sql
AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT lc.ref_text_uid COLLATE "C"
                      ORDER BY lc.ref_text_uid COLLATE "C")
                FILTER (WHERE lc.ref_text_uid IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT (lc.ref_text_uid || '|' || lc.ref_num_key) COLLATE "C"
                      ORDER BY (lc.ref_text_uid || '|' || lc.ref_num_key) COLLATE "C")
                FILTER (WHERE lc.ref_num_key IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN legal_citation lc ON lc.decision_id = t.id
        GROUP BY t.id
    ),
    upd AS (
        UPDATE decision_chunks c
        SET legal_instruments       = agg.instruments_arr,
            legal_article_composite = agg.composite_arr
        FROM agg
        WHERE c.decision_id = agg.decision_id
          AND (
                c.legal_instruments       IS DISTINCT FROM agg.instruments_arr
             OR c.legal_article_composite IS DISTINCT FROM agg.composite_arr
          )
        RETURNING 1
    )
    SELECT count(*) FROM upd;
$$;

-- 0110_decision_links.sql
CREATE TABLE decision_links (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    decision_id BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    link_type TEXT NOT NULL CHECK (
        link_type IN ('APPEL_DE', 'POURVOI_CONTRE', 'RENVOI_APRES_CASSATION')
    ),
    target_ref TEXT NOT NULL,
    target_decision_id BIGINT REFERENCES decisions(id) ON DELETE SET NULL,
    extract_version SMALLINT NOT NULL,
    UNIQUE (decision_id, link_type, target_ref)
);

CREATE INDEX decision_links_pending_target_idx
    ON decision_links (target_ref) WHERE target_decision_id IS NULL;

CREATE INDEX decision_links_target_decision_idx
    ON decision_links (target_decision_id) WHERE target_decision_id IS NOT NULL;

-- 0111_legal_text_slug_unique.sql
UPDATE legal_text t
SET slug = t.slug || '-' || lower(t.text_uid)
WHERE t.slug IS NOT NULL
  AND EXISTS (
    SELECT 1 FROM legal_text o
    WHERE o.slug = t.slug AND o.text_uid < t.text_uid
  );

DROP INDEX IF EXISTS idx_legal_text_slug;
CREATE UNIQUE INDEX idx_legal_text_slug ON legal_text (slug)
WHERE slug IS NOT NULL;

-- 0112_fix_jurisdiction_labels.sql
UPDATE jurisdiction SET label = 'Tribunal judiciaire de Rennes', city = 'Rennes'
WHERE code = 'tj35238';
UPDATE jurisdiction SET label = 'Tribunal judiciaire de Fort-de-France', city = 'Fort-de-France'
WHERE code = 'tj97209';
UPDATE jurisdiction SET label = 'Tribunal judiciaire d''Oloron-Sainte-Marie', city = 'Oloron-Sainte-Marie'
WHERE code = 'tj64445';
UPDATE jurisdiction SET label = 'Tribunal judiciaire de Châlons-en-Champagne', city = 'Châlons-en-Champagne'
WHERE code = 'tj51108';
UPDATE jurisdiction SET label = 'Tribunal de commerce d''Angoulême', city = 'Angoulême'
WHERE code = 'tcom1601';
UPDATE jurisdiction SET label = 'Tribunal de commerce de Chambéry', city = 'Chambéry'
WHERE code = 'tcom7301';
UPDATE jurisdiction SET label = 'Tribunal de commerce d''Évry', city = 'Évry'
WHERE code = 'tcom7801';
UPDATE jurisdiction SET label = 'Tribunal de commerce de Châlons-en-Champagne', city = 'Châlons-en-Champagne'
WHERE code = 'tcom5101';
UPDATE jurisdiction SET label = 'Tribunal administratif de Nouvelle-Calédonie', city = 'Nouvelle-Calédonie'
WHERE code = 'ta_nouvelle_caledonie';

UPDATE decisions SET jurisdiction_code = 'ta_amiens' WHERE jurisdiction_code = 'ta_d_amiens';
UPDATE decisions SET jurisdiction_code = 'caa_versailles' WHERE jurisdiction_code = 'caa_versailless';
UPDATE decisions SET jurisdiction_code = 'ta_reunion' WHERE jurisdiction_code = 'ta_la_reunion';
DELETE FROM jurisdiction WHERE code IN ('ta_d_amiens', 'caa_versailless', 'ta_la_reunion');

UPDATE decisions d SET jurisdiction_name = j.label
FROM jurisdiction j
WHERE j.code = d.jurisdiction_code
  AND d.jurisdiction_code IN (
    'tj35238', 'tj97209', 'tj64445', 'tj51108',
    'tcom1601', 'tcom7301', 'tcom7801', 'tcom5101',
    'ta_nouvelle_caledonie', 'ta_amiens', 'caa_versailles', 'ta_reunion'
  )
  AND d.jurisdiction_name IS DISTINCT FROM j.label;

UPDATE decision_chunks c
SET jurisdiction_name = d.jurisdiction_name, search_title = d.search_title
FROM decisions d
WHERE d.id = c.decision_id
  AND d.jurisdiction_code IN (
    'tj35238', 'tj97209', 'tj64445', 'tj51108',
    'tcom1601', 'tcom7301', 'tcom7801', 'tcom5101',
    'ta_nouvelle_caledonie', 'ta_amiens', 'caa_versailles', 'ta_reunion'
  )
  AND (c.jurisdiction_name IS DISTINCT FROM d.jurisdiction_name
       OR c.search_title IS DISTINCT FROM d.search_title);

-- 0113_case_citation.sql
CREATE TABLE case_citation (
    decision_id        int8 NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    char_start         int4 NOT NULL,
    char_end           int4 NOT NULL,
    target_ref         text NOT NULL,
    target_decision_id int8 REFERENCES decisions(id) ON DELETE SET NULL,
    extract_version    int2 NOT NULL,
    PRIMARY KEY (decision_id, char_start),
    CHECK (char_end > char_start)
);

CREATE INDEX case_citation_pending_target_idx
    ON case_citation (target_ref) WHERE target_decision_id IS NULL;

CREATE INDEX case_citation_target_decision_idx
    ON case_citation (target_decision_id) WHERE target_decision_id IS NOT NULL;
ALTER TABLE case_citation SET (fillfactor = 100,
    autovacuum_vacuum_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);

CREATE INDEX idx_decisions_docket_numbers
    ON decisions USING GIN (docket_numbers) WHERE deleted_at IS NULL;

-- 0114_facet_portee_seed_publication_relabel.sql
INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('portee:MAJEURE',      'portee', 'Majeure',      1),
    ('portee:IMPORTANTE',   'portee', 'Importante',   2),
    ('portee:LIMITEE',      'portee', 'Limitée',      3),
    ('portee:INDETERMINEE', 'portee', 'Indéterminée', 4);

UPDATE facet_value
SET label = 'Sans mention de publication'
WHERE uid = 'publication:AUTRE';

-- 0115_gin_docket_fastupdate_off.sql
ALTER INDEX idx_decisions_docket_numbers SET (fastupdate = off);
SELECT gin_clean_pending_list('idx_decisions_docket_numbers');

-- 0116_drop_unused_juridiction_facet_values.sql
DELETE FROM facet_value
WHERE uid IN ('juridiction:CE_ANALYSE', 'juridiction:CE_CONCLUSIONS');

-- 0117_decisions_chambre_formation_columns.sql
ALTER TABLE decisions
  ADD COLUMN chamber_position TEXT,
  ADD COLUMN chambre_uid   TEXT REFERENCES facet_value(uid),
  ADD COLUMN formation_uid TEXT REFERENCES facet_value(uid);

INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('chambre:CIVILE',                'chambre', 'Chambre civile',            1),
    ('chambre:SOCIALE',               'chambre', 'Chambre sociale',           2),
    ('chambre:COMMERCIALE',           'chambre', 'Chambre commerciale',       3),
    ('chambre:CRIMINELLE',            'chambre', 'Chambre criminelle',        4),
    ('chambre:CORRECTIONNELLE',       'chambre', 'Chambre correctionnelle',   5),
    ('chambre:PRUD_HOMALE',           'chambre', 'Chambre prud''homale',      6),
    ('chambre:PROTECTION_SOCIALE',    'chambre', 'Protection sociale',        7),
    ('chambre:PROCEDURES_COLLECTIVES','chambre', 'Procédures collectives',    8),
    ('chambre:INSTRUCTION',           'chambre', 'Chambre de l''instruction', 9),
    ('chambre:FAMILLE',               'chambre', 'Chambre de la famille',     10),
    ('chambre:BAUX',                  'chambre', 'Chambre des baux',          11),
    ('chambre:CONSTRUCTION',          'chambre', 'Chambre de la construction',12),
    ('chambre:ETRANGERS',             'chambre', 'Étrangers et rétention',    13),
    ('chambre:CONSEIL',               'chambre', 'Chambre du conseil',        14),
    ('chambre:EXPROPRIATION',         'chambre', 'Expropriation',             15),
    ('chambre:PROXIMITE',             'chambre', 'Proximité',                 16),
    ('chambre:SURENDETTEMENT',        'chambre', 'Surendettement',            17),
    ('chambre:COPROPRIETE',           'chambre', 'Copropriété',               18),
    ('chambre:URGENCES',              'chambre', 'Urgences',                  19),
    ('chambre:DALO',                  'chambre', 'Droit au logement (DALO)',  20),
    ('chambre:MINEURS',               'chambre', 'Chambre des mineurs',       21),
    ('chambre:NATIONALITE',           'chambre', 'Nationalité',               22),
    ('formation:A_TROIS',          'formation', 'Formation à trois',       1),
    ('formation:A_CINQ',           'formation', 'Formation à cinq',        2),
    ('formation:JUGE_UNIQUE',      'formation', 'Juge unique',             3),
    ('formation:CHAMBRE_SEULE',    'formation', 'Chambre jugeant seule',   4),
    ('formation:RESTREINTE',       'formation', 'Formation restreinte',    5),
    ('formation:SECTION',          'formation', 'Formation de section',    6),
    ('formation:PLENIERE',         'formation', 'Formation plénière',      7),
    ('formation:MIXTE',            'formation', 'Formation mixte',         8),
    ('formation:SSR',              'formation', 'Sous-sections réunies',   9),
    ('formation:CHAMBRES_REUNIES', 'formation', 'Chambres réunies',        10),
    ('formation:ASSEMBLEE',        'formation', 'Assemblée du contentieux',11),
    ('formation:SPECIALISEE',      'formation', 'Formation spécialisée',   12),
    ('office:JUGE_REFERES',                  'office', 'Juge des référés',                        8),
    ('office:PRESIDENT_SECTION_CONTENTIEUX', 'office', 'Président de la section du contentieux',  9),
    ('office:JUGE_EXPROPRIATION',            'office', 'Juge de l''expropriation',                10);

-- 0118_facet_chambre_terres_civi.sql
INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('chambre:TERRES', 'chambre', 'Chambre des terres',                23),
    ('chambre:CIVI',   'chambre', 'Indemnisation des victimes (CIVI)', 24);

