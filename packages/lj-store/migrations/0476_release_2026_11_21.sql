-- 0396_publication_language_in_key.sql
ALTER TABLE journal_issue
    ADD COLUMN language TEXT NOT NULL DEFAULT 'fr',
    ADD COLUMN status TEXT NOT NULL DEFAULT 'authentic'
        CHECK (status IN ('authentic', 'official', 'translation', 'automatic'));
ALTER TABLE text_publication
    ADD COLUMN language TEXT NOT NULL DEFAULT 'fr',
    ADD COLUMN status TEXT NOT NULL DEFAULT 'authentic'
        CHECK (status IN ('authentic', 'official', 'translation', 'automatic'));
ALTER TABLE journal_issue ALTER COLUMN language DROP DEFAULT, ALTER COLUMN status DROP DEFAULT;
ALTER TABLE text_publication ALTER COLUMN language DROP DEFAULT, ALTER COLUMN status DROP DEFAULT;

ALTER TABLE text_publication DROP CONSTRAINT text_publication_issue_uid_fkey;
ALTER TABLE journal_issue DROP CONSTRAINT journal_issue_pkey, ADD PRIMARY KEY (issue_uid, language);
ALTER TABLE text_publication DROP CONSTRAINT text_publication_pkey, ADD PRIMARY KEY (text_uid, language);
ALTER TABLE text_publication ADD FOREIGN KEY (issue_uid, language)
    REFERENCES journal_issue (issue_uid, language) ON DELETE SET NULL (issue_uid);
DROP INDEX text_publication_issue_idx;
CREATE INDEX text_publication_issue_idx ON text_publication (issue_uid, language, issue_rank);

-- 0401_decision_text_counted.sql
SET LOCAL lock_timeout = '5s';

ALTER TABLE decision_text ADD COLUMN counted BOOLEAN NOT NULL DEFAULT TRUE;

UPDATE decision_text SET counted = FALSE WHERE NOT lead;

CREATE OR REPLACE FUNCTION decision_text_put(p_id BIGINT, p_source_uid TEXT, p_lang TEXT, p_text TEXT, p_annex_start INTEGER)
RETURNS VOID LANGUAGE plpgsql AS $$
DECLARE
    v_original TEXT;
BEGIN
    IF (SELECT x.source_uid FROM decision_sources x
          JOIN decision_sources me ON me.source_uid = p_source_uid
         WHERE x.decision_id = p_id
           AND coalesce(x.lang, 'fra') = coalesce(me.lang, 'fra')
         ORDER BY x.source_rank DESC, x.source_uid ASC LIMIT 1) IS DISTINCT FROM p_source_uid THEN
        RETURN;
    END IF;
    SELECT d.lang INTO STRICT v_original FROM decisions d WHERE d.id = p_id;
    INSERT INTO decision_text (
        decision_id, lang, text_status, lead, counted, source_uid, full_text, annex_start, search_title,
        jurisdiction_type, jurisdiction_code, seat_paths, formation_paths, legal_instruments, legal_article_composite,
        publication_codes, legal_domain_uid, solution_uid, office_uid, procedure_uid, docket_numbers,
        ecli, date_lecture, deleted_at)
    SELECT d.id, p_lang, CASE WHEN p_lang = v_original THEN 'authentic' ELSE 'official' END, FALSE, FALSE,
           p_source_uid, p_text, p_annex_start, d.search_title,
           d.jurisdiction_type, d.jurisdiction_code, d.seat_paths, d.formation_paths, d.legal_instruments, d.legal_article_composite,
           d.publication_codes, d.legal_domain_uid, d.solution_uid, d.office_uid, d.procedure_uid, d.docket_numbers,
           d.ecli, d.date_lecture, d.deleted_at
      FROM decisions d WHERE d.id = p_id
    ON CONFLICT (decision_id, lang) DO UPDATE SET
        text_status = EXCLUDED.text_status,
        source_uid = EXCLUDED.source_uid,
        full_text = EXCLUDED.full_text,
        annex_start = EXCLUDED.annex_start
     WHERE (decision_text.text_status, decision_text.source_uid, decision_text.full_text, decision_text.annex_start)
           IS DISTINCT FROM (EXCLUDED.text_status, EXCLUDED.source_uid, EXCLUDED.full_text, EXCLUDED.annex_start);
    UPDATE decision_text SET lead = TRUE, counted = TRUE
     WHERE id = (SELECT min(x.id) FROM decision_text x WHERE x.decision_id = p_id)
       AND NOT EXISTS (SELECT 1 FROM decision_text x WHERE x.decision_id = p_id AND x.lead);
END $$;

-- 0402_decision_text_bm25_counted.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY decision_text_bm25_counted ON public.decision_text USING bm25 (id, decision_id, full_text, search_title, ((jurisdiction_type)::pdb.literal), ((legal_instruments)::pdb.literal), ((legal_article_composite)::pdb.literal), ((publication_codes)::pdb.literal), ((jurisdiction_code)::pdb.literal), ((seat_paths)::pdb.literal), ((formation_paths)::pdb.literal), ((legal_domain_uid)::pdb.literal), ((solution_uid)::pdb.literal), ((lj_docket_keys(docket_numbers))::pdb.literal('alias=docket_keys')), ((lj_docket_search_keys(docket_numbers))::pdb.literal('alias=docket_search')), ((ecli)::pdb.literal_normalized('alias=ecli')), date_lecture, deleted_at, counted, ((counted)::pdb.alias('lead')), annex_start, ((lj_text_before_annexes(full_text, annex_start))::pdb.regex_pattern('\p{N}+(?:[./-]\p{N}+)+|[\p{L}\p{N}-]+', 'ascii_folding=true', 'stopwords_language=French', 'alias=text_before_annexes')), ((lj_text_annexes(full_text, annex_start))::pdb.regex_pattern('\p{N}+(?:[./-]\p{N}+)+|[\p{L}\p{N}-]+', 'ascii_folding=true', 'stopwords_language=French', 'alias=annexes')), ((lj_summary_quote(summary, summary_quote_start, summary_quote_end))::pdb.regex_pattern('\p{N}+(?:[./-]\p{N}+)+|[\p{L}\p{N}-]+', 'ascii_folding=true', 'stopwords_language=French', 'alias=official_summary'))) WITH (key_field=id, text_fields='{"full_text": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}, "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}}');

-- 0403_decision_text_bm25_counted_swap.sql
DO $$ BEGIN IF NOT (SELECT indisvalid FROM pg_index WHERE indexrelid = 'decision_text_bm25_counted'::regclass) THEN RAISE EXCEPTION 'decision_text_bm25_counted is not valid'; END IF; END $$;
DROP INDEX decision_text_bm25;
ALTER INDEX decision_text_bm25_counted RENAME TO decision_text_bm25;

-- 0404_decision_text_summary_stock_counted.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY idx_decision_text_summary_stock_counted
ON decision_text (summary_prompt_version, summary_engine) WHERE counted AND deleted_at IS NULL;

-- 0405_decision_text_drop_lead.sql
SET LOCAL lock_timeout = '5s';

CREATE OR REPLACE FUNCTION decision_text_put(p_id BIGINT, p_source_uid TEXT, p_lang TEXT, p_text TEXT, p_annex_start INTEGER)
RETURNS VOID LANGUAGE plpgsql AS $$
DECLARE
    v_original TEXT;
BEGIN
    IF (SELECT x.source_uid FROM decision_sources x
          JOIN decision_sources me ON me.source_uid = p_source_uid
         WHERE x.decision_id = p_id
           AND coalesce(x.lang, 'fra') = coalesce(me.lang, 'fra')
         ORDER BY x.source_rank DESC, x.source_uid ASC LIMIT 1) IS DISTINCT FROM p_source_uid THEN
        RETURN;
    END IF;
    SELECT d.lang INTO STRICT v_original FROM decisions d WHERE d.id = p_id;
    INSERT INTO decision_text (
        decision_id, lang, text_status, counted, source_uid, full_text, annex_start, search_title,
        jurisdiction_type, jurisdiction_code, seat_paths, formation_paths, legal_instruments, legal_article_composite,
        publication_codes, legal_domain_uid, solution_uid, office_uid, procedure_uid, docket_numbers,
        ecli, date_lecture, deleted_at)
    SELECT d.id, p_lang, CASE WHEN p_lang = v_original THEN 'authentic' ELSE 'official' END, FALSE,
           p_source_uid, p_text, p_annex_start, d.search_title,
           d.jurisdiction_type, d.jurisdiction_code, d.seat_paths, d.formation_paths, d.legal_instruments, d.legal_article_composite,
           d.publication_codes, d.legal_domain_uid, d.solution_uid, d.office_uid, d.procedure_uid, d.docket_numbers,
           d.ecli, d.date_lecture, d.deleted_at
      FROM decisions d WHERE d.id = p_id
    ON CONFLICT (decision_id, lang) DO UPDATE SET
        text_status = EXCLUDED.text_status,
        source_uid = EXCLUDED.source_uid,
        full_text = EXCLUDED.full_text,
        annex_start = EXCLUDED.annex_start
     WHERE (decision_text.text_status, decision_text.source_uid, decision_text.full_text, decision_text.annex_start)
           IS DISTINCT FROM (EXCLUDED.text_status, EXCLUDED.source_uid, EXCLUDED.full_text, EXCLUDED.annex_start);
    UPDATE decision_text SET counted = TRUE
     WHERE id = (SELECT min(x.id) FROM decision_text x WHERE x.decision_id = p_id)
       AND NOT EXISTS (SELECT 1 FROM decision_text x WHERE x.decision_id = p_id AND x.counted);
END $$;

ALTER TABLE decision_text DROP COLUMN lead;

ALTER INDEX idx_decision_text_summary_stock_counted RENAME TO idx_decision_text_summary_stock;

-- 0406_text_publication_id_and_printed_text.sql
SET LOCAL lock_timeout = '5s';
ALTER TABLE text_publication ADD COLUMN id BIGINT GENERATED BY DEFAULT AS IDENTITY;
CREATE UNIQUE INDEX text_publication_id_key ON text_publication (id);
CREATE OR REPLACE FUNCTION lj_publication_text(blocks JSONB, sections JSONB, articles JSONB) RETURNS TEXT
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
SELECT nullif(concat_ws(E'\n',
    (SELECT string_agg(b.value, E'\n') FROM jsonb_each_text(blocks) b),
    (SELECT string_agg(s.value ->> 'title', E'\n') FROM jsonb_each(sections) s),
    (SELECT string_agg(a.value ->> 'text', E'\n') FROM jsonb_each(articles) a)), '') $$;

-- 0407_text_publication_bm25.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY text_publication_bm25 ON public.text_publication USING bm25 (
    id,
    body,
    ((coalesce(title_full, title))::pdb.regex_pattern('\p{N}+(?:[./-]\p{N}+)+|[\p{L}\p{N}-]+', 'ascii_folding=true', 'stopwords_language=French', 'alias=heading')),
    ((lj_publication_text(blocks, sections, articles))::pdb.regex_pattern('\p{N}+(?:[./-]\p{N}+)+|[\p{L}\p{N}-]+', 'ascii_folding=true', 'stopwords_language=French', 'alias=printed')),
    ((text_uid)::pdb.literal), ((language)::pdb.literal), ((status)::pdb.literal),
    ((journal_code)::pdb.literal), ((nature)::pdb.literal), ((issue_uid)::pdb.literal),
    date_publi
) WITH (key_field=id, text_fields='{"body": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}}');

-- 0408_notice_slug.sql
CREATE TABLE notice_slug (slug TEXT PRIMARY KEY, notice_uid TEXT NOT NULL);

-- 0409_decision_source_expired_checksum.sql
ALTER TABLE decision_sources ADD COLUMN expired_checksum TEXT;

-- 0410_amf_source_rank.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN CASE WHEN source_fields->'sections' @> '[{"kind":"dispositif"}]' THEN 60 ELSE 59 END
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ddd'          THEN 50
            WHEN 'adlc'         THEN 50
            WHEN 'comeu'        THEN 50
            WHEN 'amf'          THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            WHEN 'contribution' THEN 10
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

INSERT INTO facet_value (uid, facet, label, abbr, sort) VALUES
    ('jurisdiction_type:AMF', 'jurisdiction_type', 'Autorité des marchés financiers', 'AMF', 16);

-- 0411_decision_text_one_language.sql
SET LOCAL lock_timeout = '5s';

CREATE OR REPLACE FUNCTION decision_text_put(p_id BIGINT, p_source_uid TEXT, p_lang TEXT, p_text TEXT, p_annex_start INTEGER)
RETURNS VOID LANGUAGE plpgsql AS $$
DECLARE
    v_original TEXT;
BEGIN
    DELETE FROM decision_text WHERE decision_id = p_id AND source_uid = p_source_uid AND lang <> p_lang;
    IF (SELECT x.source_uid FROM decision_sources x
          JOIN decision_sources me ON me.source_uid = p_source_uid
         WHERE x.decision_id = p_id
           AND coalesce(x.lang, 'fra') = coalesce(me.lang, 'fra')
         ORDER BY x.source_rank DESC, x.source_uid ASC LIMIT 1) IS DISTINCT FROM p_source_uid THEN
        RETURN;
    END IF;
    SELECT d.lang INTO STRICT v_original FROM decisions d WHERE d.id = p_id;
    INSERT INTO decision_text (
        decision_id, lang, text_status, counted, source_uid, full_text, annex_start, search_title,
        jurisdiction_type, jurisdiction_code, seat_paths, formation_paths, legal_instruments, legal_article_composite,
        publication_codes, legal_domain_uid, solution_uid, office_uid, procedure_uid, docket_numbers,
        ecli, date_lecture, deleted_at)
    SELECT d.id, p_lang, CASE WHEN p_lang = v_original THEN 'authentic' ELSE 'official' END, FALSE,
           p_source_uid, p_text, p_annex_start, d.search_title,
           d.jurisdiction_type, d.jurisdiction_code, d.seat_paths, d.formation_paths, d.legal_instruments, d.legal_article_composite,
           d.publication_codes, d.legal_domain_uid, d.solution_uid, d.office_uid, d.procedure_uid, d.docket_numbers,
           d.ecli, d.date_lecture, d.deleted_at
      FROM decisions d WHERE d.id = p_id
    ON CONFLICT (decision_id, lang) DO UPDATE SET
        text_status = EXCLUDED.text_status,
        source_uid = EXCLUDED.source_uid,
        full_text = EXCLUDED.full_text,
        annex_start = EXCLUDED.annex_start
     WHERE (decision_text.text_status, decision_text.source_uid, decision_text.full_text, decision_text.annex_start)
           IS DISTINCT FROM (EXCLUDED.text_status, EXCLUDED.source_uid, EXCLUDED.full_text, EXCLUDED.annex_start);
    UPDATE decision_text SET counted = TRUE
     WHERE id = (SELECT min(x.id) FROM decision_text x WHERE x.decision_id = p_id)
       AND NOT EXISTS (SELECT 1 FROM decision_text x WHERE x.decision_id = p_id AND x.counted);
END $$;

-- 0412_columns_store_raw.sql
DO $$
DECLARE r record;
BEGIN
	FOR r IN SELECT c.oid::regclass AS rel, c.relkind, a.attname
		FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid JOIN pg_namespace n ON n.oid = c.relnamespace
		WHERE n.nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast') AND c.relkind IN ('r', 'p', 'm')
		AND a.attnum > 0 AND NOT a.attisdropped AND a.attstorage IN ('x', 'm') LOOP
		EXECUTE format('ALTER %s %s ALTER COLUMN %I SET STORAGE EXTERNAL',
			CASE r.relkind WHEN 'm' THEN 'MATERIALIZED VIEW' ELSE 'TABLE' END, r.rel, r.attname);
	END LOOP;
END $$;

CREATE OR REPLACE FUNCTION public.lj_columns_store_raw() RETURNS event_trigger LANGUAGE plpgsql AS $$
DECLARE r record;
BEGIN
	FOR r IN SELECT DISTINCT c.oid::regclass AS rel, c.relkind, a.attname
		FROM pg_event_trigger_ddl_commands() d JOIN pg_class c ON c.oid = d.objid
		JOIN pg_attribute a ON a.attrelid = c.oid
		WHERE d.classid = 'pg_class'::regclass AND c.relkind IN ('r', 'p', 'm')
		AND c.relnamespace <> 'pg_toast'::regnamespace
		AND a.attnum > 0 AND NOT a.attisdropped AND a.attstorage IN ('x', 'm') LOOP
		EXECUTE format('ALTER %s %s ALTER COLUMN %I SET STORAGE EXTERNAL',
			CASE r.relkind WHEN 'm' THEN 'MATERIALIZED VIEW' ELSE 'TABLE' END, r.rel, r.attname);
	END LOOP;
END $$;

DROP EVENT TRIGGER IF EXISTS lj_columns_store_raw;
CREATE EVENT TRIGGER lj_columns_store_raw ON ddl_command_end
	WHEN TAG IN ('CREATE TABLE', 'CREATE TABLE AS', 'SELECT INTO', 'ALTER TABLE', 'CREATE MATERIALIZED VIEW')
	EXECUTE FUNCTION public.lj_columns_store_raw();

-- 0413_drop_bodacc_officer_bm25.sql
-- lj:sans-transaction
DROP INDEX CONCURRENTLY IF EXISTS bodacc_officer_bm25;

-- 0414_juricaf_source_rank.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN CASE WHEN source_fields->'sections' @> '[{"kind":"dispositif"}]' THEN 60 ELSE 59 END
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ddd'          THEN 50
            WHEN 'adlc'         THEN 50
            WHEN 'comeu'        THEN 50
            WHEN 'amf'          THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            WHEN 'juricaf'      THEN 20
            WHEN 'contribution' THEN 10
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0415_foreign_court_code_underscore.sql
SET LOCAL lock_timeout = '5s';

INSERT INTO jurisdiction (code, jurisdiction_type, city, label, source_code, place, closed_on, successor_code)
SELECT replace(code, '-', '_'), jurisdiction_type, city, label, replace(source_code, '-', '_'),
       place, closed_on, successor_code
FROM jurisdiction
WHERE code LIKE '%-%'
ON CONFLICT DO NOTHING;

UPDATE decisions
SET jurisdiction_code = replace(jurisdiction_code, '-', '_'),
    seat = jsonb_set(seat, '{jurisdiction_code}', to_jsonb(replace(jurisdiction_code, '-', '_'))),
    seat_paths = ARRAY(SELECT replace(p, '-', '_') FROM unnest(seat_paths) AS p)
WHERE jurisdiction_code LIKE '%-%';

UPDATE jurisdiction_year_counts
SET jurisdiction_code = replace(jurisdiction_code, '-', '_')
WHERE jurisdiction_code LIKE '%-%';

DELETE FROM jurisdiction WHERE code LIKE '%-%';

-- 0416_entscheidsuche_source_rank.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN CASE WHEN source_fields->'sections' @> '[{"kind":"dispositif"}]' THEN 60 ELSE 59 END
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ddd'          THEN 50
            WHEN 'adlc'         THEN 50
            WHEN 'comeu'        THEN 50
            WHEN 'amf'          THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            WHEN 'entscheidsuche' THEN 30
            WHEN 'juricaf'      THEN 20
            WHEN 'contribution' THEN 10
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0417_juradm_site_source_rank.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN CASE WHEN source_fields->'sections' @> '[{"kind":"dispositif"}]' THEN 60 ELSE 59 END
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ddd'          THEN 50
            WHEN 'adlc'         THEN 50
            WHEN 'comeu'        THEN 50
            WHEN 'amf'          THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'juradm-site'  THEN 40
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            WHEN 'entscheidsuche' THEN 30
            WHEN 'juricaf'      THEN 20
            WHEN 'contribution' THEN 10
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0418_drop_user_bookmarks.sql
SET LOCAL lock_timeout = '5s';

DROP TABLE comptes.user_bookmarks;

-- 0419_payload_format_doc_odt.sql
SET LOCAL lock_timeout = '5s';

ALTER TABLE decision_full_text
  DROP CONSTRAINT decision_full_text_payload_format_check,
  ADD CONSTRAINT decision_full_text_payload_format_check
    CHECK (payload_format IN ('xml', 'json', 'dila-xml', 'html', 'pdf', 'docx', 'doc', 'odt')) NOT VALID;

ALTER TABLE decision_sources
  DROP CONSTRAINT decision_sources_payload_format_check,
  ADD CONSTRAINT decision_sources_payload_format_check
    CHECK (payload_format IN ('xml', 'json', 'dila-xml', 'html', 'pdf', 'docx', 'doc', 'odt')) NOT VALID;

-- 0420_legal_article_text_copies.sql
SET LOCAL lock_timeout = '5s';

CREATE FUNCTION lj_article_searchable(slug text, role text, status text, num_key text)
  RETURNS boolean LANGUAGE sql IMMUTABLE PARALLEL SAFE
  RETURN slug IS NOT NULL
    AND role NOT IN ('individuel', 'vehicule', 'habilitation')
    AND (num_key <> 'body' OR coalesce(status, 'VIGUEUR') = 'VIGUEUR');

CREATE FUNCTION lj_article_text_copies() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  SELECT t.title, t.jurisdiction, upper(t.nature), t.slug,
         lj_article_searchable(t.slug, t.role, t.status, NEW.num_key),
         t.text_date, t.jurisdiction_paths
    INTO NEW.code_title, NEW.jurisdiction, NEW.nature, NEW.slug,
         NEW.searchable, NEW.text_date, NEW.jurisdiction_paths
    FROM legal_text t
   WHERE t.text_uid = NEW.text_uid;
  RETURN NEW;
END $$;

CREATE TRIGGER legal_article_text_copies
  BEFORE INSERT OR UPDATE OF text_uid, num_key, code_title, jurisdiction, nature, slug,
                             searchable, text_date, jurisdiction_paths
  ON legal_article FOR EACH ROW EXECUTE FUNCTION lj_article_text_copies();

CREATE FUNCTION lj_text_article_copies() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  UPDATE legal_article a SET code_title = a.code_title
    FROM legal_text t
   WHERE t.text_uid = NEW.text_uid AND a.text_uid = t.text_uid
     AND (a.code_title IS DISTINCT FROM t.title
       OR a.jurisdiction IS DISTINCT FROM t.jurisdiction
       OR a.nature IS DISTINCT FROM upper(t.nature)
       OR a.slug IS DISTINCT FROM t.slug
       OR a.searchable IS DISTINCT FROM lj_article_searchable(t.slug, t.role, t.status, a.num_key)
       OR a.text_date IS DISTINCT FROM t.text_date
       OR a.jurisdiction_paths IS DISTINCT FROM t.jurisdiction_paths);
  RETURN NULL;
END $$;

CREATE CONSTRAINT TRIGGER legal_text_article_copies_insert
  AFTER INSERT ON legal_text DEFERRABLE INITIALLY DEFERRED
  FOR EACH ROW EXECUTE FUNCTION lj_text_article_copies();

CREATE CONSTRAINT TRIGGER legal_text_article_copies_update
  AFTER UPDATE ON legal_text DEFERRABLE INITIALLY DEFERRED
  FOR EACH ROW
  WHEN (OLD.text_uid IS DISTINCT FROM NEW.text_uid
     OR OLD.title IS DISTINCT FROM NEW.title
     OR OLD.jurisdiction IS DISTINCT FROM NEW.jurisdiction
     OR OLD.nature IS DISTINCT FROM NEW.nature
     OR OLD.slug IS DISTINCT FROM NEW.slug
     OR OLD.role IS DISTINCT FROM NEW.role
     OR OLD.status IS DISTINCT FROM NEW.status
     OR OLD.text_date IS DISTINCT FROM NEW.text_date
     OR OLD.jurisdiction_paths IS DISTINCT FROM NEW.jurisdiction_paths)
  EXECUTE FUNCTION lj_text_article_copies();

-- 0421_cc_site_source_rank.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN CASE WHEN source_fields->'sections' @> '[{"kind":"dispositif"}]' THEN 60 ELSE 59 END
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ddd'          THEN 50
            WHEN 'adlc'         THEN 50
            WHEN 'comeu'        THEN 50
            WHEN 'amf'          THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'juradm-site'  THEN 40
            WHEN 'cc-site'      THEN 40
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            WHEN 'entscheidsuche' THEN 30
            WHEN 'juricaf'      THEN 20
            WHEN 'contribution' THEN 10
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0424_consiliaweb_source_rank.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN CASE WHEN source_fields->'sections' @> '[{"kind":"dispositif"}]' THEN 60 ELSE 59 END
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ddd'          THEN 50
            WHEN 'adlc'         THEN 50
            WHEN 'comeu'        THEN 50
            WHEN 'amf'          THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'juradm-site'  THEN 40
            WHEN 'cc-site'      THEN 40
            WHEN 'consiliaweb'  THEN 40
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            WHEN 'entscheidsuche' THEN 30
            WHEN 'juricaf'      THEN 20
            WHEN 'contribution' THEN 10
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0425_legal_text_bm25_without_body.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY legal_text_bm25 ON public.legal_text USING bm25 (id, title, ((text_uid)::pdb.literal), ((nature)::pdb.literal), ((jurisdiction)::pdb.literal), ((slug)::pdb.literal), ((role)::pdb.literal), ((COALESCE(status, 'VIGUEUR'::text))::pdb.literal('alias=status_eff')), ((upper(nature))::pdb.literal('alias=nature_up')), ((lower(nature))::pdb.literal('alias=source')), text_date, ((jurisdiction_paths)::pdb.literal)) WITH (key_field=id, text_fields='{
    "title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}
  }');

-- 0426_drop_ce_avis_court.sql
DELETE FROM jurisdiction_year_counts WHERE jurisdiction_code = 'ce_avis';
DELETE FROM jurisdiction WHERE code = 'ce_avis';

-- 0427_drop_legal_text_body.sql
DROP INDEX legal_text_body_bm25;
ALTER TABLE legal_text DROP COLUMN body;

-- 0428_legal_text_parties.sql
ALTER TABLE legal_text ADD COLUMN parties TEXT[];

-- 0429_drop_legal_text_body_again.sql
ALTER TABLE legal_text DROP COLUMN IF EXISTS body;

-- 0430_official_foreign_sources_rank.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN CASE WHEN source_fields->'sections' @> '[{"kind":"dispositif"}]' THEN 60 ELSE 59 END
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ddd'          THEN 50
            WHEN 'adlc'         THEN 50
            WHEN 'comeu'        THEN 50
            WHEN 'amf'          THEN 50
            WHEN 'legimonaco'   THEN 50
            WHEN 'justice-lu'   THEN 50
            WHEN 'scc-csc'      THEN 50
            WHEN 'juportal'     THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'juradm-site'  THEN 40
            WHEN 'cc-site'      THEN 40
            WHEN 'consiliaweb'  THEN 40
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            WHEN 'entscheidsuche' THEN 30
            WHEN 'juricaf'      THEN 20
            WHEN 'contribution' THEN 10
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

INSERT INTO facet_value (uid, facet, label, abbr, parent_uid, sort) VALUES
    ('geo:CA', 'geo', 'Canada', NULL, NULL, 178);

-- 0431_legalis_source_rank.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN CASE WHEN source_fields->'sections' @> '[{"kind":"dispositif"}]' THEN 60 ELSE 59 END
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ddd'          THEN 50
            WHEN 'adlc'         THEN 50
            WHEN 'comeu'        THEN 50
            WHEN 'amf'          THEN 50
            WHEN 'legimonaco'   THEN 50
            WHEN 'justice-lu'   THEN 50
            WHEN 'scc-csc'      THEN 50
            WHEN 'juportal'     THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'juradm-site'  THEN 40
            WHEN 'cc-site'      THEN 40
            WHEN 'consiliaweb'  THEN 40
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            WHEN 'entscheidsuche' THEN 30
            WHEN 'juricaf'      THEN 20
            WHEN 'contribution' THEN 10
            WHEN 'legalis'      THEN 5
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0432_criminal_trial_court_types.sql
INSERT INTO facet_value (uid, facet, label, abbr, sort) VALUES
    ('jurisdiction_type:ASSISES', 'jurisdiction_type', 'Cour d''assises', 'C. assises', 30),
    ('jurisdiction_type:CCD',     'jurisdiction_type', 'Cour criminelle départementale', 'CCD', 31),
    ('formation:APPEL',           'formation',         'Statuant en appel', NULL, 14),
    ('formation:SPECIALE',        'formation',         'Spécialement composée', NULL, 15);

-- 0433_ddd_jurisprudence_source_rank.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN CASE WHEN source_fields->'sections' @> '[{"kind":"dispositif"}]' THEN 60 ELSE 59 END
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ddd'          THEN 50
            WHEN 'adlc'         THEN 50
            WHEN 'comeu'        THEN 50
            WHEN 'amf'          THEN 50
            WHEN 'legimonaco'   THEN 50
            WHEN 'justice-lu'   THEN 50
            WHEN 'scc-csc'      THEN 50
            WHEN 'juportal'     THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'juradm-site'  THEN 40
            WHEN 'cc-site'      THEN 40
            WHEN 'consiliaweb'  THEN 40
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            WHEN 'entscheidsuche' THEN 30
            WHEN 'juricaf'      THEN 20
            WHEN 'contribution' THEN 10
            WHEN 'ddd-jurisprudence' THEN 8
            WHEN 'legalis'      THEN 5
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0434_legal_article_bm25_v2_texte_b.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY legal_article_bm25_v2 ON public.legal_article USING bm25 (id, search_title, texte, num, ((source)::pdb.literal), ((text_uid)::pdb.literal), ((status)::pdb.literal), ((jurisdiction)::pdb.literal), ((nature)::pdb.literal), ((slug)::pdb.literal), ((kind)::pdb.literal), searchable, date_debut, date_fin, text_date, ((jurisdiction_paths)::pdb.literal)) WITH (key_field=id, text_fields='{
    "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "texte":        {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq", "b": 0.1},
    "num":          {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "basic"}
  }');

-- 0435_legal_article_bm25_swap_texte_b.sql
DROP INDEX legal_article_bm25;

ALTER INDEX legal_article_bm25_v2 RENAME TO legal_article_bm25;

-- 0436_solution_roots_and_leaves.sql
INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('solution:GRANTED',        'solution', 'Fondé',              1),
    ('solution:PARTLY_GRANTED', 'solution', 'Fondé en partie',    2),
    ('solution:DISMISSED',      'solution', 'Non fondé',          3),
    ('solution:INADMISSIBLE',   'solution', 'Irrecevable',        4),
    ('solution:CLOSED',         'solution', 'Clos sans jugement', 5),
    ('solution:OTHER',          'solution', 'Autre',              6);

INSERT INTO facet_value (uid, facet, label) VALUES
    ('solution:VIOLATION',            'solution', 'Violation'),
    ('solution:ANNULATION_PARTIELLE', 'solution', 'Annulation partielle'),
    ('solution:NON_VIOLATION',        'solution', 'Non-violation'),
    ('solution:NON_ADMISSION',        'solution', 'Non-admission'),
    ('solution:DECHEANCE',            'solution', 'Déchéance'),
    ('solution:REGLEMENT_AMIABLE',    'solution', 'Règlement amiable'),
    ('solution:RADIATION',            'solution', 'Radiation'),
    ('solution:RECEVABLE',            'solution', 'Recevable');

UPDATE facet_value f SET parent_uid = v.root, sort = v.sort FROM (VALUES
    ('solution:SATISFACTION_TOTALE',    'solution:GRANTED',        11),
    ('solution:CASSATION',              'solution:GRANTED',        12),
    ('solution:ANNULATION',             'solution:GRANTED',        13),
    ('solution:INFIRMATION',            'solution:GRANTED',        14),
    ('solution:NON_CONFORMITE',         'solution:GRANTED',        15),
    ('solution:INELIGIBILITE',          'solution:GRANTED',        16),
    ('solution:VIOLATION',              'solution:GRANTED',        17),
    ('solution:SATISFACTION_PARTIELLE', 'solution:PARTLY_GRANTED', 21),
    ('solution:CASSATION_PARTIELLE',    'solution:PARTLY_GRANTED', 22),
    ('solution:ANNULATION_PARTIELLE',   'solution:PARTLY_GRANTED', 23),
    ('solution:INFIRMATION_PARTIELLE',  'solution:PARTLY_GRANTED', 24),
    ('solution:REFORMATION',            'solution:PARTLY_GRANTED', 25),
    ('solution:REJET',                  'solution:DISMISSED',      31),
    ('solution:CONFIRMATION',           'solution:DISMISSED',      32),
    ('solution:CONFORMITE',             'solution:DISMISSED',      33),
    ('solution:NON_VIOLATION',          'solution:DISMISSED',      34),
    ('solution:IRRECEVABILITE',         'solution:INADMISSIBLE',   41),
    ('solution:NON_ADMISSION',          'solution:INADMISSIBLE',   42),
    ('solution:DECHEANCE',              'solution:INADMISSIBLE',   43),
    ('solution:DESISTEMENT',            'solution:CLOSED',         51),
    ('solution:NON_LIEU_A_STATUER',     'solution:CLOSED',         52),
    ('solution:REGLEMENT_AMIABLE',      'solution:CLOSED',         53),
    ('solution:RADIATION',              'solution:CLOSED',         54),
    ('solution:AUTRE',                  'solution:OTHER',          61),
    ('solution:RECEVABLE',              'solution:OTHER',          62)
) AS v(leaf, root, sort)
WHERE f.uid = v.leaf;

-- 0437_legal_domain_union_and_new_leaves.sql
INSERT INTO facet_value (uid, facet, label, parent_uid, sort) VALUES
    ('legal_domain:CIVIL_DROIT_INTERNATIONAL_PRIVE',     'legal_domain', 'Droit international privé',                                       'legal_domain:CIVIL',    16),
    ('legal_domain:PUBLIC_DROIT_LIBERTES_FONDAMENTALES', 'legal_domain', 'Libertés fondamentales',                                          'legal_domain:PUBLIC',    7),
    ('legal_domain:PUBLIC_DROIT_COMMANDE_PUBLIQUE',      'legal_domain', 'Droit de la commande publique',                                   'legal_domain:PUBLIC',    8),
    ('legal_domain:FISCAL_DROIT_DOUANIER',               'legal_domain', 'Droit douanier',                                                  'legal_domain:FISCAL',    1),
    ('legal_domain:EUROPEEN_LIBERTES_CIRCULATION',       'legal_domain', 'Marché intérieur et libertés de circulation',                     'legal_domain:EUROPEEN',  1),
    ('legal_domain:EUROPEEN_AIDES_ETAT',                 'legal_domain', 'Aides d''État',                                                   'legal_domain:EUROPEEN',  2),
    ('legal_domain:EUROPEEN_POLITIQUE_AGRICOLE',         'legal_domain', 'Politique agricole commune et pêche',                             'legal_domain:EUROPEEN',  3),
    ('legal_domain:EUROPEEN_ACTION_EXTERIEURE',          'legal_domain', 'Politique commerciale, action extérieure et mesures restrictives', 'legal_domain:EUROPEEN',  4),
    ('legal_domain:EUROPEEN_DROIT_INSTITUTIONNEL',       'legal_domain', 'Droit institutionnel de l''Union',                                'legal_domain:EUROPEEN',  5);

UPDATE facet_value SET label = 'Droit de l''Union européenne'                     WHERE uid = 'legal_domain:EUROPEEN';
UPDATE facet_value SET label = 'Droit du numérique et des données personnelles' WHERE uid = 'legal_domain:COMMERCIAL_DROIT_NUMERIQUE';
UPDATE facet_value SET label = 'Aide et protection sociale'                      WHERE uid = 'legal_domain:SOCIAL_DROIT_AIDE_ACTION_SOCIALE';

-- 0438_solution_admission_leaves.sql
INSERT INTO facet_value (uid, facet, label, parent_uid, sort) VALUES
    ('solution:ADMISSION',           'solution', 'Admission',           'solution:GRANTED',        18),
    ('solution:ADMISSION_PARTIELLE', 'solution', 'Admission partielle', 'solution:PARTLY_GRANTED', 26);

-- 0439_conclusions.sql
CREATE TABLE conclusions (
    id                BIGSERIAL PRIMARY KEY,
    source            TEXT NOT NULL,
    source_uid        TEXT NOT NULL UNIQUE,
    jurisdiction_code TEXT NOT NULL,
    case_numbers      TEXT[] NOT NULL,
    ruled_on          DATE,
    delivered_on      DATE,
    author            TEXT,
    langs             TEXT[] NOT NULL DEFAULT '{}',
    source_fields     JSONB NOT NULL DEFAULT '{}',
    public_id         TEXT UNIQUE,
    summary           TEXT,
    summary_engine    SMALLINT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX conclusions_case_numbers ON conclusions USING gin (case_numbers);

-- 0440_norm_visit.sql
CREATE TABLE norm_visit (
    source           TEXT NOT NULL,
    item             TEXT NOT NULL,
    visited_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    content_checksum BIGINT NOT NULL,
    PRIMARY KEY (source, item)
);

-- 0441_decision_links_cedh_request.sql
ALTER TABLE decision_links DROP CONSTRAINT decision_links_link_type_check;
ALTER TABLE decision_links ADD CONSTRAINT decision_links_link_type_check
    CHECK (link_type IN ('APPEL_DE', 'POURVOI_CONTRE', 'RENVOI_APRES_CASSATION', 'REQUETE_CEDH_CONTRE')) NOT VALID;
ALTER TABLE decision_links VALIDATE CONSTRAINT decision_links_link_type_check;
ALTER TABLE decision_links ADD COLUMN visited_at TIMESTAMPTZ;

-- 0442_cedh_application.sql
CREATE TABLE cedh_application (
    itemid             TEXT PRIMARY KEY,
    appnos             TEXT[] NOT NULL,
    name               TEXT NOT NULL,
    introduced_on      DATE,
    communicated_on    DATE NOT NULL,
    articles           TEXT[] NOT NULL,
    text               TEXT NOT NULL,
    target_court       TEXT,
    target_date        DATE,
    target_decision_id int8 REFERENCES decisions(id) ON DELETE SET NULL,
    visited_at         TIMESTAMPTZ
);

CREATE INDEX cedh_application_target_decision_idx ON cedh_application (target_decision_id);

-- 0443_cce_source_rank.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN CASE WHEN source_fields->'sections' @> '[{"kind":"dispositif"}]' THEN 60 ELSE 59 END
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ddd'          THEN 50
            WHEN 'adlc'         THEN 50
            WHEN 'comeu'        THEN 50
            WHEN 'amf'          THEN 50
            WHEN 'legimonaco'   THEN 50
            WHEN 'justice-lu'   THEN 50
            WHEN 'scc-csc'      THEN 50
            WHEN 'juportal'     THEN 50
            WHEN 'cce'          THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'juradm-site'  THEN 40
            WHEN 'cc-site'      THEN 40
            WHEN 'consiliaweb'  THEN 40
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            WHEN 'entscheidsuche' THEN 30
            WHEN 'juricaf'      THEN 20
            WHEN 'contribution' THEN 10
            WHEN 'ddd-jurisprudence' THEN 8
            WHEN 'legalis'      THEN 5
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0444_legal_text_alias.sql
CREATE TABLE legal_text_alias (
    text_uid TEXT NOT NULL REFERENCES legal_text (text_uid) ON UPDATE CASCADE ON DELETE CASCADE,
    kind     TEXT NOT NULL CHECK (kind IN ('abbreviation', 'number', 'title')),
    lang     TEXT NOT NULL CHECK (lang ~ '^([a-z]{2})?$' AND (kind = 'number') = (lang = '')),
    value    TEXT NOT NULL CHECK (value <> ''),
    PRIMARY KEY (text_uid, kind, lang, value)
);

-- 0445_legal_article_passage.sql
CREATE TABLE legal_article_passage (
    article_id BIGINT NOT NULL REFERENCES legal_article (id) ON DELETE CASCADE,
    passage_index SMALLINT NOT NULL,
    content_checksum BIGINT NOT NULL,
    embedding rabitq4(512) NOT NULL,
    PRIMARY KEY (article_id, passage_index)
);

-- 0446_african_court_sites_source_rank.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN CASE WHEN source_fields->'sections' @> '[{"kind":"dispositif"}]' THEN 60 ELSE 59 END
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ddd'          THEN 50
            WHEN 'adlc'         THEN 50
            WHEN 'comeu'        THEN 50
            WHEN 'amf'          THEN 50
            WHEN 'legimonaco'   THEN 50
            WHEN 'justice-lu'   THEN 50
            WHEN 'scc-csc'      THEN 50
            WHEN 'juportal'     THEN 50
            WHEN 'cce'          THEN 50
            WHEN 'cc-bj'        THEN 50
            WHEN 'hcc-mg'       THEN 50
            WHEN 'jurisprudence-mg' THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'juradm-site'  THEN 40
            WHEN 'cc-site'      THEN 40
            WHEN 'consiliaweb'  THEN 40
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            WHEN 'entscheidsuche' THEN 30
            WHEN 'juricaf'      THEN 20
            WHEN 'contribution' THEN 10
            WHEN 'ddd-jurisprudence' THEN 8
            WHEN 'legalis'      THEN 5
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0447_autorite_administrative_type.sql
INSERT INTO facet_value (uid, facet, label, abbr, sort) VALUES
    ('jurisdiction_type:AUTORITE_ADMINISTRATIVE', 'jurisdiction_type', 'Autorité administrative', NULL, 32);

-- 0448_parliament_store.sql
CREATE TABLE parliament_dossier (
    uid              TEXT PRIMARY KEY,
    aliases          TEXT[] NOT NULL DEFAULT '{}',
    title            TEXT NOT NULL,
    kind             TEXT NOT NULL,
    state            TEXT NOT NULL,
    opened           DATE,
    closed           DATE,
    law_text_uid     TEXT,
    source           TEXT NOT NULL,
    content_checksum BIGINT NOT NULL,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX parliament_dossier_aliases ON parliament_dossier USING gin (aliases);
CREATE INDEX parliament_dossier_law ON parliament_dossier (law_text_uid) WHERE law_text_uid IS NOT NULL;

CREATE TABLE parliament_document (
    uid              TEXT PRIMARY KEY,
    aliases          TEXT[] NOT NULL DEFAULT '{}',
    chamber          TEXT NOT NULL,
    kind             TEXT NOT NULL,
    legislature      SMALLINT,
    session          TEXT,
    number           TEXT,
    title            TEXT,
    analysis         TEXT,
    authors          JSONB NOT NULL DEFAULT '[]',
    ministry         TEXT,
    dated            DATE,
    printings        JSONB NOT NULL DEFAULT '[]',
    url              TEXT,
    dossier_uid      TEXT REFERENCES parliament_dossier (uid) ON DELETE SET NULL,
    fate             TEXT,
    text_uid         TEXT REFERENCES legal_text (text_uid) ON DELETE SET NULL,
    source           TEXT NOT NULL,
    source_uid       TEXT NOT NULL,
    content_checksum BIGINT NOT NULL,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX parliament_document_aliases ON parliament_document USING gin (aliases);
CREATE INDEX parliament_document_number ON parliament_document (chamber, kind, legislature, number);
CREATE INDEX parliament_document_dossier ON parliament_document (dossier_uid) WHERE dossier_uid IS NOT NULL;
CREATE INDEX parliament_document_text ON parliament_document (text_uid) WHERE text_uid IS NOT NULL;

CREATE TABLE parliament_passage (
    document_uid     TEXT NOT NULL REFERENCES parliament_document (uid) ON DELETE CASCADE,
    rank             INT NOT NULL,
    zone             TEXT NOT NULL,
    label            TEXT,
    bill_article     TEXT,
    amendments       TEXT[] NOT NULL DEFAULT '{}',
    speakers         JSONB NOT NULL DEFAULT '[]',
    jo_date          DATE,
    jo_page          INT,
    heading          TEXT,
    text             TEXT NOT NULL,
    content_checksum BIGINT NOT NULL,
    PRIMARY KEY (document_uid, rank)
);

CREATE TABLE parliament_bill_article (
    dossier_uid      TEXT NOT NULL REFERENCES parliament_dossier (uid) ON DELETE CASCADE,
    label            TEXT NOT NULL,
    law_text_uid     TEXT NOT NULL,
    law_num          TEXT NOT NULL,
    PRIMARY KEY (dossier_uid, label)
);
CREATE INDEX parliament_bill_article_law ON parliament_bill_article (law_text_uid, law_num);

CREATE TABLE parliament_passage_citation (
    document_uid     TEXT NOT NULL,
    rank             INT NOT NULL,
    char_start       INT NOT NULL,
    char_end         INT NOT NULL,
    ref_text_uid     TEXT NOT NULL REFERENCES legal_text (text_uid) ON DELETE CASCADE,
    ref_num_key      TEXT,
    extract_version  SMALLINT NOT NULL,
    PRIMARY KEY (document_uid, rank, char_start),
    FOREIGN KEY (document_uid, rank) REFERENCES parliament_passage (document_uid, rank) ON DELETE CASCADE,
    CHECK (char_end > char_start)
);
CREATE INDEX parliament_passage_citation_ref ON parliament_passage_citation (ref_text_uid, ref_num_key);

-- 0450_opencaselaw_source_rank.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN CASE WHEN source_fields->'sections' @> '[{"kind":"dispositif"}]' THEN 60 ELSE 59 END
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ddd'          THEN 50
            WHEN 'adlc'         THEN 50
            WHEN 'comeu'        THEN 50
            WHEN 'amf'          THEN 50
            WHEN 'legimonaco'   THEN 50
            WHEN 'justice-lu'   THEN 50
            WHEN 'scc-csc'      THEN 50
            WHEN 'juportal'     THEN 50
            WHEN 'cce'          THEN 50
            WHEN 'cc-bj'        THEN 50
            WHEN 'hcc-mg'       THEN 50
            WHEN 'jurisprudence-mg' THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'juradm-site'  THEN 40
            WHEN 'cc-site'      THEN 40
            WHEN 'consiliaweb'  THEN 40
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            WHEN 'entscheidsuche' THEN 30
            WHEN 'opencaselaw'  THEN 30
            WHEN 'juricaf'      THEN 20
            WHEN 'contribution' THEN 10
            WHEN 'ddd-jurisprudence' THEN 8
            WHEN 'legalis'      THEN 5
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0452_cc_ci_source_rank.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN CASE WHEN source_fields->'sections' @> '[{"kind":"dispositif"}]' THEN 60 ELSE 59 END
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ddd'          THEN 50
            WHEN 'adlc'         THEN 50
            WHEN 'comeu'        THEN 50
            WHEN 'amf'          THEN 50
            WHEN 'legimonaco'   THEN 50
            WHEN 'justice-lu'   THEN 50
            WHEN 'scc-csc'      THEN 50
            WHEN 'juportal'     THEN 50
            WHEN 'cce'          THEN 50
            WHEN 'cc-bj'        THEN 50
            WHEN 'cc-ci'        THEN 50
            WHEN 'hcc-mg'       THEN 50
            WHEN 'jurisprudence-mg' THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'juradm-site'  THEN 40
            WHEN 'cc-site'      THEN 40
            WHEN 'consiliaweb'  THEN 40
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            WHEN 'entscheidsuche' THEN 30
            WHEN 'opencaselaw'  THEN 30
            WHEN 'juricaf'      THEN 20
            WHEN 'contribution' THEN 10
            WHEN 'ddd-jurisprudence' THEN 8
            WHEN 'legalis'      THEN 5
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0454_slug_redirect_jurisdiction.sql
SET LOCAL lock_timeout = '5s';

ALTER TABLE legal_text_slug_redirect ADD COLUMN jurisdiction text;
UPDATE legal_text_slug_redirect r SET jurisdiction = t.jurisdiction
  FROM legal_text t WHERE t.text_uid = r.text_uid;
ALTER TABLE legal_text_slug_redirect
  ALTER COLUMN jurisdiction SET NOT NULL,
  DROP CONSTRAINT legal_text_slug_redirect_pkey,
  ADD PRIMARY KEY (ancien_slug, jurisdiction);

-- 0455_african_court_place_and_rank.sql
SET LOCAL lock_timeout = '5s';

INSERT INTO facet_value (uid, facet, label, abbr, parent_uid, sort) VALUES
    ('geo:UNIONAFRICAINE', 'geo', 'Union africaine', 'UA', NULL, 179);

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN CASE WHEN source_fields->'sections' @> '[{"kind":"dispositif"}]' THEN 60 ELSE 59 END
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ddd'          THEN 50
            WHEN 'adlc'         THEN 50
            WHEN 'comeu'        THEN 50
            WHEN 'amf'          THEN 50
            WHEN 'legimonaco'   THEN 50
            WHEN 'justice-lu'   THEN 50
            WHEN 'scc-csc'      THEN 50
            WHEN 'juportal'     THEN 50
            WHEN 'cce'          THEN 50
            WHEN 'cc-bj'        THEN 50
            WHEN 'african-court' THEN 50
            WHEN 'cc-ci'        THEN 50
            WHEN 'hcc-mg'       THEN 50
            WHEN 'jurisprudence-mg' THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'juradm-site'  THEN 40
            WHEN 'cc-site'      THEN 40
            WHEN 'consiliaweb'  THEN 40
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            WHEN 'entscheidsuche' THEN 30
            WHEN 'opencaselaw'  THEN 30
            WHEN 'juricaf'      THEN 20
            WHEN 'contribution' THEN 10
            WHEN 'ddd-jurisprudence' THEN 8
            WHEN 'legalis'      THEN 5
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0457_jurisdiction_names.sql
ALTER TABLE jurisdiction ADD COLUMN names jsonb;

-- 0458_ccja_cassation_ci_source_rank.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN CASE WHEN source_fields->'sections' @> '[{"kind":"dispositif"}]' THEN 60 ELSE 59 END
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ddd'          THEN 50
            WHEN 'adlc'         THEN 50
            WHEN 'comeu'        THEN 50
            WHEN 'amf'          THEN 50
            WHEN 'legimonaco'   THEN 50
            WHEN 'justice-lu'   THEN 50
            WHEN 'scc-csc'      THEN 50
            WHEN 'juportal'     THEN 50
            WHEN 'cce'          THEN 50
            WHEN 'cc-bj'        THEN 50
            WHEN 'african-court' THEN 50
            WHEN 'ccja'         THEN 50
            WHEN 'cassation-ci' THEN 50
            WHEN 'cc-ci'        THEN 50
            WHEN 'hcc-mg'       THEN 50
            WHEN 'jurisprudence-mg' THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'juradm-site'  THEN 40
            WHEN 'cc-site'      THEN 40
            WHEN 'consiliaweb'  THEN 40
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            WHEN 'entscheidsuche' THEN 30
            WHEN 'opencaselaw'  THEN 30
            WHEN 'juricaf'      THEN 20
            WHEN 'contribution' THEN 10
            WHEN 'ddd-jurisprudence' THEN 8
            WHEN 'legalis'      THEN 5
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0459_african_member_state_places.sql
SET LOCAL lock_timeout = '5s';

INSERT INTO facet_value (uid, facet, label, abbr, parent_uid, sort) VALUES
    ('geo:EH', 'geo', 'Sahara occidental', NULL, NULL, 180),
    ('geo:GH', 'geo', 'Ghana', NULL, NULL, 181),
    ('geo:GM', 'geo', 'Gambie', NULL, NULL, 182),
    ('geo:GQ', 'geo', 'Guinée équatoriale', NULL, NULL, 183),
    ('geo:GW', 'geo', 'Guinée-Bissau', NULL, NULL, 184),
    ('geo:KE', 'geo', 'Kenya', NULL, NULL, 185),
    ('geo:LR', 'geo', 'Libéria', NULL, NULL, 186),
    ('geo:LS', 'geo', 'Lesotho', NULL, NULL, 187),
    ('geo:LY', 'geo', 'Libye', NULL, NULL, 188),
    ('geo:MW', 'geo', 'Malawi', NULL, NULL, 189),
    ('geo:MZ', 'geo', 'Mozambique', NULL, NULL, 190),
    ('geo:SL', 'geo', 'Sierra Leone', NULL, NULL, 191),
    ('geo:TZ', 'geo', 'Tanzanie', NULL, NULL, 192),
    ('geo:UG', 'geo', 'Ouganda', NULL, NULL, 193),
    ('geo:ZA', 'geo', 'Afrique du Sud', NULL, NULL, 194),
    ('geo:ZM', 'geo', 'Zambie', NULL, NULL, 195);

-- 0460_legal_article_text.sql
CREATE TABLE legal_article_text (
    article_id BIGINT NOT NULL REFERENCES legal_article (id) ON DELETE CASCADE,
    language TEXT NOT NULL,
    text_status TEXT NOT NULL CHECK (text_status IN ('authentic', 'official', 'translation', 'automatic')),
    texte TEXT NOT NULL,
    title_path TEXT,
    content_checksum BIGINT NOT NULL,
    PRIMARY KEY (article_id, language)
);

ALTER TABLE legal_article ADD COLUMN language TEXT NOT NULL DEFAULT 'fr';

-- 0461_decision_text_reader_summary.sql
SET LOCAL lock_timeout = '5s';

CREATE TABLE decision_text_reader_summary (
    decision_text_id        BIGINT NOT NULL REFERENCES decision_text(id) ON DELETE CASCADE,
    lang                    TEXT NOT NULL CHECK (lang ~ '^[a-z]{2}$'),
    summary                 TEXT NOT NULL,
    summary_engine          SMALLINT NOT NULL,
    summary_prompt_version  SMALLINT NOT NULL,
    summary_body_start      INTEGER,
    summary_body_end        INTEGER,
    PRIMARY KEY (decision_text_id, lang)
);

-- 0462_rechtspraak_source_rank.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN CASE WHEN source_fields->'sections' @> '[{"kind":"dispositif"}]' THEN 60 ELSE 59 END
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ddd'          THEN 50
            WHEN 'adlc'         THEN 50
            WHEN 'comeu'        THEN 50
            WHEN 'amf'          THEN 50
            WHEN 'legimonaco'   THEN 50
            WHEN 'justice-lu'   THEN 50
            WHEN 'scc-csc'      THEN 50
            WHEN 'juportal'     THEN 50
            WHEN 'rechtspraak'  THEN 50
            WHEN 'cce'          THEN 50
            WHEN 'cc-bj'        THEN 50
            WHEN 'african-court' THEN 50
            WHEN 'ccja'         THEN 50
            WHEN 'cassation-ci' THEN 50
            WHEN 'cc-ci'        THEN 50
            WHEN 'hcc-mg'       THEN 50
            WHEN 'jurisprudence-mg' THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'juradm-site'  THEN 40
            WHEN 'cc-site'      THEN 40
            WHEN 'consiliaweb'  THEN 40
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            WHEN 'entscheidsuche' THEN 30
            WHEN 'opencaselaw'  THEN 30
            WHEN 'juricaf'      THEN 20
            WHEN 'contribution' THEN 10
            WHEN 'ddd-jurisprudence' THEN 8
            WHEN 'legalis'      THEN 5
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0463_raa_document_bm25.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY raa_document_bm25 ON public.raa_document USING bm25 (
    id,
    title,
    body,
    ((publisher)::pdb.literal), ((dept)::pdb.literal), ((status)::pdb.literal),
    published
) WITH (key_field=id, text_fields='{"title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}, "body": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}}');

-- 0464_cndj_ci_source_rank.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN CASE WHEN source_fields->'sections' @> '[{"kind":"dispositif"}]' THEN 60 ELSE 59 END
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ddd'          THEN 50
            WHEN 'adlc'         THEN 50
            WHEN 'comeu'        THEN 50
            WHEN 'amf'          THEN 50
            WHEN 'legimonaco'   THEN 50
            WHEN 'justice-lu'   THEN 50
            WHEN 'scc-csc'      THEN 50
            WHEN 'juportal'     THEN 50
            WHEN 'rechtspraak'  THEN 50
            WHEN 'cce'          THEN 50
            WHEN 'cc-bj'        THEN 50
            WHEN 'african-court' THEN 50
            WHEN 'ccja'         THEN 50
            WHEN 'cassation-ci' THEN 50
            WHEN 'cndj-ci'      THEN 50
            WHEN 'cc-ci'        THEN 50
            WHEN 'hcc-mg'       THEN 50
            WHEN 'jurisprudence-mg' THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'juradm-site'  THEN 40
            WHEN 'cc-site'      THEN 40
            WHEN 'consiliaweb'  THEN 40
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            WHEN 'entscheidsuche' THEN 30
            WHEN 'opencaselaw'  THEN 30
            WHEN 'juricaf'      THEN 20
            WHEN 'contribution' THEN 10
            WHEN 'ddd-jurisprudence' THEN 8
            WHEN 'legalis'      THEN 5
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0465_caribbean_places.sql
SET LOCAL lock_timeout = '5s';

INSERT INTO facet_value (uid, facet, label, abbr, parent_uid, sort) VALUES
    ('geo:AN', 'geo', 'Antilles néerlandaises', NULL, NULL, 196),
    ('geo:AW', 'geo', 'Aruba', NULL, NULL, 197),
    ('geo:BQ', 'geo', 'Bonaire, Saint-Eustache et Saba', NULL, NULL, 198),
    ('geo:CW', 'geo', 'Curaçao', NULL, NULL, 199),
    ('geo:SX', 'geo', 'Saint-Martin (Pays-Bas)', NULL, NULL, 200),
    ('geo:GHVJ', 'geo', 'Gemeenschappelijk Hof van Justitie van Aruba, Curaçao, Sint Maarten en van Bonaire, Sint Eustatius en Saba', 'GHvJ', NULL, 201);

-- 0466_decisions_kind.sql
SET LOCAL lock_timeout = '5s';

ALTER TABLE decisions ADD COLUMN kind TEXT NOT NULL DEFAULT 'decision';
ALTER TABLE decision_text ADD COLUMN kind TEXT NOT NULL DEFAULT 'decision';
ALTER TABLE decision_chunks ADD COLUMN kind TEXT NOT NULL DEFAULT 'decision';

CREATE OR REPLACE FUNCTION decision_text_put(p_id bigint, p_source_uid text, p_lang text, p_text text, p_annex_start integer)
 RETURNS void
 LANGUAGE plpgsql
AS $function$
DECLARE
    v_original TEXT;
BEGIN
    DELETE FROM decision_text WHERE decision_id = p_id AND source_uid = p_source_uid AND lang <> p_lang;
    IF (SELECT x.source_uid FROM decision_sources x
          JOIN decision_sources me ON me.source_uid = p_source_uid
         WHERE x.decision_id = p_id
           AND coalesce(x.lang, 'fra') = coalesce(me.lang, 'fra')
         ORDER BY x.source_rank DESC, x.source_uid ASC LIMIT 1) IS DISTINCT FROM p_source_uid THEN
        RETURN;
    END IF;
    SELECT d.lang INTO STRICT v_original FROM decisions d WHERE d.id = p_id;
    INSERT INTO decision_text (
        decision_id, lang, text_status, counted, source_uid, full_text, annex_start, search_title,
        jurisdiction_type, jurisdiction_code, seat_paths, formation_paths, legal_instruments, legal_article_composite,
        publication_codes, legal_domain_uid, solution_uid, office_uid, procedure_uid, docket_numbers,
        ecli, date_lecture, deleted_at, kind)
    SELECT d.id, p_lang, CASE WHEN p_lang = v_original THEN 'authentic' ELSE 'official' END, FALSE,
           p_source_uid, p_text, p_annex_start, d.search_title,
           d.jurisdiction_type, d.jurisdiction_code, d.seat_paths, d.formation_paths, d.legal_instruments, d.legal_article_composite,
           d.publication_codes, d.legal_domain_uid, d.solution_uid, d.office_uid, d.procedure_uid, d.docket_numbers,
           d.ecli, d.date_lecture, d.deleted_at, d.kind
      FROM decisions d WHERE d.id = p_id
    ON CONFLICT (decision_id, lang) DO UPDATE SET
        text_status = EXCLUDED.text_status,
        source_uid = EXCLUDED.source_uid,
        full_text = EXCLUDED.full_text,
        annex_start = EXCLUDED.annex_start
     WHERE (decision_text.text_status, decision_text.source_uid, decision_text.full_text, decision_text.annex_start)
           IS DISTINCT FROM (EXCLUDED.text_status, EXCLUDED.source_uid, EXCLUDED.full_text, EXCLUDED.annex_start);
    UPDATE decision_text SET counted = TRUE
     WHERE id = (SELECT min(x.id) FROM decision_text x WHERE x.decision_id = p_id)
       AND NOT EXISTS (SELECT 1 FROM decision_text x WHERE x.decision_id = p_id AND x.counted);
END $function$;

CREATE OR REPLACE FUNCTION decision_text_mirror(p_ids bigint[])
 RETURNS void
 LANGUAGE sql
AS $function$
    UPDATE decision_text t SET
        search_title = d.search_title,
        jurisdiction_type = d.jurisdiction_type,
        jurisdiction_code = d.jurisdiction_code,
        seat_paths = d.seat_paths,
        formation_paths = d.formation_paths,
        legal_instruments = d.legal_instruments,
        legal_article_composite = d.legal_article_composite,
        publication_codes = d.publication_codes,
        legal_domain_uid = d.legal_domain_uid,
        solution_uid = d.solution_uid,
        office_uid = d.office_uid,
        procedure_uid = d.procedure_uid,
        docket_numbers = d.docket_numbers,
        ecli = d.ecli,
        date_lecture = d.date_lecture,
        deleted_at = d.deleted_at,
        kind = d.kind
      FROM decisions d
     WHERE d.id = ANY(p_ids) AND t.decision_id = d.id
       AND (t.search_title, t.jurisdiction_type, t.jurisdiction_code, t.seat_paths, t.formation_paths, t.legal_instruments,
            t.legal_article_composite, t.publication_codes, t.legal_domain_uid, t.solution_uid, t.office_uid,
            t.procedure_uid, t.docket_numbers, t.ecli, t.date_lecture, t.deleted_at, t.kind)
           IS DISTINCT FROM
           (d.search_title, d.jurisdiction_type, d.jurisdiction_code, d.seat_paths, d.formation_paths, d.legal_instruments,
            d.legal_article_composite, d.publication_codes, d.legal_domain_uid, d.solution_uid, d.office_uid,
            d.procedure_uid, d.docket_numbers, d.ecli, d.date_lecture, d.deleted_at, d.kind);
$function$;

DROP TRIGGER decisions_mirror_text ON decisions;

CREATE CONSTRAINT TRIGGER decisions_mirror_text
    AFTER INSERT OR UPDATE OF search_title, lang,
        jurisdiction_type, jurisdiction_code, seat_paths, formation_paths, legal_instruments, legal_article_composite,
        publication_codes, legal_domain_uid, solution_uid, office_uid, procedure_uid, docket_numbers,
        ecli, date_lecture, deleted_at, kind
    ON decisions DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0467_decision_text_bm25_kind.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY decision_text_bm25_kind ON public.decision_text USING bm25 (id, decision_id, full_text, search_title, ((jurisdiction_type)::pdb.literal), ((legal_instruments)::pdb.literal), ((legal_article_composite)::pdb.literal), ((publication_codes)::pdb.literal), ((jurisdiction_code)::pdb.literal), ((seat_paths)::pdb.literal), ((formation_paths)::pdb.literal), ((legal_domain_uid)::pdb.literal), ((solution_uid)::pdb.literal), ((kind)::pdb.literal), ((lj_docket_keys(docket_numbers))::pdb.literal('alias=docket_keys')), ((lj_docket_search_keys(docket_numbers))::pdb.literal('alias=docket_search')), ((ecli)::pdb.literal_normalized('alias=ecli')), date_lecture, deleted_at, counted, annex_start, ((lj_text_before_annexes(full_text, annex_start))::pdb.regex_pattern('\p{N}+(?:[./-]\p{N}+)+|[\p{L}\p{N}-]+', 'ascii_folding=true', 'stopwords_language=French', 'alias=text_before_annexes')), ((lj_text_annexes(full_text, annex_start))::pdb.regex_pattern('\p{N}+(?:[./-]\p{N}+)+|[\p{L}\p{N}-]+', 'ascii_folding=true', 'stopwords_language=French', 'alias=annexes')), ((lj_summary_quote(summary, summary_quote_start, summary_quote_end))::pdb.regex_pattern('\p{N}+(?:[./-]\p{N}+)+|[\p{L}\p{N}-]+', 'ascii_folding=true', 'stopwords_language=French', 'alias=official_summary'))) WITH (key_field=id, text_fields='{"full_text": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}, "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}}');

-- 0468_decision_text_bm25_kind_swap.sql
DO $$ BEGIN IF NOT (SELECT indisvalid FROM pg_index WHERE indexrelid = 'decision_text_bm25_kind'::regclass) THEN RAISE EXCEPTION 'decision_text_bm25_kind is not valid'; END IF; END $$;
DROP INDEX decision_text_bm25;
ALTER INDEX decision_text_bm25_kind RENAME TO decision_text_bm25;

-- 0469_djibouti_council_source_rank.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN CASE WHEN source_fields->'sections' @> '[{"kind":"dispositif"}]' THEN 60 ELSE 59 END
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ddd'          THEN 50
            WHEN 'adlc'         THEN 50
            WHEN 'comeu'        THEN 50
            WHEN 'amf'          THEN 50
            WHEN 'legimonaco'   THEN 50
            WHEN 'justice-lu'   THEN 50
            WHEN 'scc-csc'      THEN 50
            WHEN 'juportal'     THEN 50
            WHEN 'rechtspraak'  THEN 50
            WHEN 'cce'          THEN 50
            WHEN 'cc-bj'        THEN 50
            WHEN 'african-court' THEN 50
            WHEN 'ccja'         THEN 50
            WHEN 'cassation-ci' THEN 50
            WHEN 'cndj-ci'      THEN 50
            WHEN 'cc-ci'        THEN 50
            WHEN 'hcc-mg'       THEN 50
            WHEN 'jurisprudence-mg' THEN 50
            WHEN 'journalofficiel-dj' THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'juradm-site'  THEN 40
            WHEN 'cc-site'      THEN 40
            WHEN 'cc-dj'        THEN 40
            WHEN 'consiliaweb'  THEN 40
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            WHEN 'entscheidsuche' THEN 30
            WHEN 'opencaselaw'  THEN 30
            WHEN 'juricaf'      THEN 20
            WHEN 'contribution' THEN 10
            WHEN 'ddd-jurisprudence' THEN 8
            WHEN 'legalis'      THEN 5
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0470_norm_slug_decision.sql
CREATE TABLE norm_slug_decision (
    slug TEXT PRIMARY KEY,
    decision_id BIGINT NOT NULL REFERENCES decisions (id) ON DELETE CASCADE
);

-- 0471_cc_ma_source_rank.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN CASE WHEN source_fields->'sections' @> '[{"kind":"dispositif"}]' THEN 60 ELSE 59 END
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ddd'          THEN 50
            WHEN 'adlc'         THEN 50
            WHEN 'comeu'        THEN 50
            WHEN 'amf'          THEN 50
            WHEN 'legimonaco'   THEN 50
            WHEN 'justice-lu'   THEN 50
            WHEN 'scc-csc'      THEN 50
            WHEN 'juportal'     THEN 50
            WHEN 'rechtspraak'  THEN 50
            WHEN 'cce'          THEN 50
            WHEN 'cc-bj'        THEN 50
            WHEN 'african-court' THEN 50
            WHEN 'ccja'         THEN 50
            WHEN 'cassation-ci' THEN 50
            WHEN 'cndj-ci'      THEN 50
            WHEN 'cc-ma'        THEN 50
            WHEN 'cc-ci'        THEN 50
            WHEN 'hcc-mg'       THEN 50
            WHEN 'jurisprudence-mg' THEN 50
            WHEN 'journalofficiel-dj' THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'juradm-site'  THEN 40
            WHEN 'cc-site'      THEN 40
            WHEN 'cc-dj'        THEN 40
            WHEN 'consiliaweb'  THEN 40
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            WHEN 'entscheidsuche' THEN 30
            WHEN 'opencaselaw'  THEN 30
            WHEN 'juricaf'      THEN 20
            WHEN 'contribution' THEN 10
            WHEN 'ddd-jurisprudence' THEN 8
            WHEN 'legalis'      THEN 5
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0472_a2aj_source_rank.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN CASE WHEN source_fields->'sections' @> '[{"kind":"dispositif"}]' THEN 60 ELSE 59 END
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ddd'          THEN 50
            WHEN 'adlc'         THEN 50
            WHEN 'comeu'        THEN 50
            WHEN 'amf'          THEN 50
            WHEN 'legimonaco'   THEN 50
            WHEN 'justice-lu'   THEN 50
            WHEN 'scc-csc'      THEN 50
            WHEN 'juportal'     THEN 50
            WHEN 'rechtspraak'  THEN 50
            WHEN 'cce'          THEN 50
            WHEN 'cc-bj'        THEN 50
            WHEN 'african-court' THEN 50
            WHEN 'ccja'         THEN 50
            WHEN 'cassation-ci' THEN 50
            WHEN 'cndj-ci'      THEN 50
            WHEN 'cc-ma'        THEN 50
            WHEN 'cc-ci'        THEN 50
            WHEN 'hcc-mg'       THEN 50
            WHEN 'jurisprudence-mg' THEN 50
            WHEN 'journalofficiel-dj' THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'juradm-site'  THEN 40
            WHEN 'cc-site'      THEN 40
            WHEN 'cc-dj'        THEN 40
            WHEN 'consiliaweb'  THEN 40
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            WHEN 'entscheidsuche' THEN 30
            WHEN 'opencaselaw'  THEN 30
            WHEN 'a2aj'         THEN 30
            WHEN 'juricaf'      THEN 20
            WHEN 'contribution' THEN 10
            WHEN 'ddd-jurisprudence' THEN 8
            WHEN 'legalis'      THEN 5
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0473_canada_province_places.sql
SET LOCAL lock_timeout = '5s';

INSERT INTO facet_value (uid, facet, label, abbr, parent_uid, sort) VALUES
    ('geo:CA-NS', 'geo', 'Nouvelle-Écosse', 'NS', 'geo:CA', 236),
    ('geo:CA-NL', 'geo', 'Terre-Neuve-et-Labrador', 'NL', 'geo:CA', 237);

-- 0474_ontario_british_columbia_places.sql
SET LOCAL lock_timeout = '5s';

INSERT INTO facet_value (uid, facet, label, abbr, parent_uid, sort) VALUES
    ('geo:CA-ON', 'geo', 'Ontario', 'ON', 'geo:CA', 238),
    ('geo:CA-BC', 'geo', 'Colombie-Britannique', 'BC', 'geo:CA', 239);

-- 0475_yukon_place.sql
SET LOCAL lock_timeout = '5s';

INSERT INTO facet_value (uid, facet, label, abbr, parent_uid, sort) VALUES
    ('geo:CA-YT', 'geo', 'Yukon', 'YT', 'geo:CA', 240);

-- 0476_quebec_new_brunswick_places.sql
SET LOCAL lock_timeout = '5s';

INSERT INTO facet_value (uid, facet, label, abbr, parent_uid, sort) VALUES
    ('geo:CA-QC', 'geo', 'Québec', 'QC', 'geo:CA', 241),
    ('geo:CA-NB', 'geo', 'Nouveau-Brunswick', 'NB', 'geo:CA', 242);

