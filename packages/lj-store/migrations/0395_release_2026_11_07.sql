-- 0339_drop_account_activity_history.sql

DROP TABLE comptes.user_decision_views;
DROP TABLE comptes.user_search_history;
ALTER TABLE comptes.users DROP COLUMN track_activity;

-- 0340_dila_inca_rank_jurisdiction_successor.sql

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN 60
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

ALTER TABLE jurisdiction
    ADD COLUMN closed_on DATE,
    ADD COLUMN successor_code TEXT REFERENCES jurisdiction (code);

INSERT INTO facet_value (uid, facet, label, abbr, sort) VALUES
    ('jurisdiction_type:CPH', 'jurisdiction_type', 'Conseil de prud''hommes', 'CPH', 14);

-- 0341_decision_actors.sql
CREATE TABLE decision_actors (
    decision_id     bigint   PRIMARY KEY REFERENCES decisions(id) ON DELETE CASCADE,
    extract_version smallint NOT NULL,
    actors          jsonb    NOT NULL
);

CREATE FUNCTION public.lj_actor_terms(actors jsonb) RETURNS text[]
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT
AS $fn$
SELECT coalesce(array_agg(DISTINCT t.term), '{}')
FROM jsonb_array_elements(actors) AS el,
     LATERAL (SELECT CASE
                 WHEN el ? 'e' THEN 'e|' || (el->>'e')
                 WHEN el->>'q' IN ('p', 'l', 'c') THEN 'k|' || (el->>'k')
              END AS term) AS t
WHERE t.term IS NOT NULL
$fn$;

CREATE INDEX decision_actors_terms_idx ON decision_actors
    USING gin (public.lj_actor_terms(actors)) WITH (fastupdate = off);

CREATE FUNCTION public.lj_actor_set_entities(actors jsonb, ords int[], uids text[])
RETURNS jsonb
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS $fn$
SELECT coalesce(jsonb_agg(CASE
           WHEN u.ord IS NULL THEN x.el
           WHEN u.uid IS NULL THEN x.el - 'e'
           ELSE jsonb_set(x.el, '{e}', to_jsonb(u.uid))
       END ORDER BY x.n), '[]')
FROM jsonb_array_elements(actors) WITH ORDINALITY AS x(el, n)
LEFT JOIN (SELECT DISTINCT ON (ord) ord, uid
           FROM unnest(ords, uids) AS u(ord, uid)) AS u
       ON u.ord = x.n - 1
$fn$;

CREATE VIEW decision_actor AS
SELECT a.decision_id,
       (x.n - 1)::int AS ord,
       CASE x.el->>'q'
           WHEN 'p' THEN 'party' WHEN 'l' THEN 'law_firm'
           WHEN 'c' THEN 'counsel_name' WHEN 'i' THEN 'intervenor'
       END AS quality,
       CASE x.el->>'s' WHEN 'a' THEN 'applicant' WHEN 'd' THEN 'defendant' END AS side,
       x.el->>'v' AS value,
       x.el->>'k' AS resolve_key,
       x.el->>'e' AS entity_uid,
       CASE x.el->>'n'
           WHEN 'ph' THEN 'physique' WHEN 'pr' THEN 'morale_privee'
           WHEN 'pu' THEN 'morale_publique'
       END AS nature,
       x.el->>'b' AS barreau,
       x.el->>'r' AS role,
       (x.el->>'f')::int AS firm_ord,
       (x.el->>'p')::int AS party_ord,
       (x.el->>'o')::int AS sub_ord,
       x.el->'m' AS mentions,
       a.extract_version
FROM decision_actors a
CROSS JOIN LATERAL jsonb_array_elements(a.actors) WITH ORDINALITY AS x(el, n);

-- 0342_decision_actor_rows_and_links.sql
CREATE FUNCTION public.lj_actor_rows(actors jsonb)
RETURNS TABLE (
    ord         int,
    quality     text,
    side        text,
    value       text,
    resolve_key text,
    entity_uid  text,
    nature      text,
    barreau     text,
    role        text,
    firm_ord    int,
    party_ord   int,
    sub_ord     int,
    mentions    jsonb
)
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS $fn$
SELECT (x.n - 1)::int,
       CASE x.el->>'q'
           WHEN 'p' THEN 'party' WHEN 'l' THEN 'law_firm'
           WHEN 'c' THEN 'counsel_name' WHEN 'i' THEN 'intervenor'
       END,
       CASE x.el->>'s' WHEN 'a' THEN 'applicant' WHEN 'd' THEN 'defendant' END,
       x.el->>'v',
       x.el->>'k',
       x.el->>'e',
       CASE x.el->>'n'
           WHEN 'ph' THEN 'physique' WHEN 'pr' THEN 'morale_privee'
           WHEN 'pu' THEN 'morale_publique'
       END,
       x.el->>'b',
       x.el->>'r',
       (x.el->>'f')::int,
       (x.el->>'p')::int,
       (x.el->>'o')::int,
       x.el->'m'
FROM jsonb_array_elements(actors) WITH ORDINALITY AS x(el, n)
$fn$;

CREATE OR REPLACE VIEW decision_actor AS
SELECT a.decision_id, r.ord, r.quality, r.side, r.value, r.resolve_key, r.entity_uid,
       r.nature, r.barreau, r.role, r.firm_ord, r.party_ord, r.sub_ord, r.mentions,
       a.extract_version
FROM decision_actors a
CROSS JOIN LATERAL public.lj_actor_rows(a.actors) AS r;

CREATE FUNCTION public.lj_actor_set_entities(actors jsonb, ords int[], keys text[], uids text[])
RETURNS jsonb
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS $fn$
SELECT coalesce(jsonb_agg(CASE
           WHEN u.ord IS NULL OR x.el->>'k' IS DISTINCT FROM u.key THEN x.el
           WHEN u.uid IS NULL THEN x.el - 'e'
           ELSE jsonb_set(x.el, '{e}', to_jsonb(u.uid))
       END ORDER BY x.n), '[]')
FROM jsonb_array_elements(actors) WITH ORDINALITY AS x(el, n)
LEFT JOIN (SELECT DISTINCT ON (ord) ord, key, uid
           FROM unnest(ords, keys, uids) AS u(ord, key, uid)) AS u
       ON u.ord = x.n - 1
$fn$;

CREATE FUNCTION public.lj_actor_remap_entities(actors jsonb, olds text[], news text[])
RETURNS jsonb
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS $fn$
SELECT coalesce(jsonb_agg(CASE
           WHEN m.old IS NULL THEN x.el
           WHEN m.new IS NULL THEN x.el - 'e'
           ELSE jsonb_set(x.el, '{e}', to_jsonb(m.new))
       END ORDER BY x.n), '[]')
FROM jsonb_array_elements(actors) WITH ORDINALITY AS x(el, n)
LEFT JOIN (SELECT DISTINCT ON (old) old, new
           FROM unnest(olds, news) AS m(old, new)) AS m
       ON m.old = x.el->>'e'
$fn$;

-- 0343_drop_decision_party_and_flat_actors.sql
SET LOCAL lock_timeout = '5s';

DROP TABLE decision_party;

ALTER TABLE decisions
    DROP COLUMN applicant_counsel_names,
    DROP COLUMN applicant_law_firms,
    DROP COLUMN applicant_companies,
    DROP COLUMN defendant_counsel_names,
    DROP COLUMN defendant_law_firms,
    DROP COLUMN defendant_companies,
    DROP COLUMN intervenors;

DROP FUNCTION public.lj_actor_set_entities(jsonb, int[], text[]);

-- 0344_journal_issue_and_text_publication.sql
CREATE TABLE journal_issue (
    issue_uid    TEXT PRIMARY KEY,
    journal_code TEXT NOT NULL,
    num          TEXT,
    date_publi   DATE,
    eli          TEXT,
    title        TEXT NOT NULL,
    toc          JSONB NOT NULL
);
CREATE INDEX journal_issue_date_idx ON journal_issue (journal_code, date_publi);

CREATE TABLE text_publication (
    text_uid      TEXT PRIMARY KEY,
    issue_uid     TEXT REFERENCES journal_issue (issue_uid) ON DELETE SET NULL,
    issue_rank    INT,
    num_parution  TEXT,
    num_sequence  INT,
    origine_publi TEXT,
    date_publi    DATE,
    eli           TEXT,
    page_deb      INT,
    page_fin      INT,
    ministere     TEXT,
    autorite      TEXT,
    title         TEXT,
    title_full    TEXT,
    keywords      TEXT[] NOT NULL DEFAULT '{}',
    enterprise    JSONB,
    blocks        JSONB NOT NULL DEFAULT '{}',
    structure     JSONB,
    sections      JSONB NOT NULL DEFAULT '{}',
    articles      JSONB NOT NULL DEFAULT '{}'
);
CREATE INDEX text_publication_issue_idx ON text_publication (issue_uid, issue_rank);

-- 0345_decisions_wikidata_id.sql
ALTER TABLE decisions ADD COLUMN wikidata_id text;

UPDATE decisions
SET case_name = NULL, case_name_keys = NULL, case_name_source = NULL
WHERE case_name_source = 'canon';

ALTER TABLE decisions DROP CONSTRAINT decisions_case_name_source_check;
ALTER TABLE decisions ADD CONSTRAINT decisions_case_name_source_check
    CHECK (case_name_source IN ('wikimedia', 'ce-list', 'source', 'renvoi')) NOT VALID;

CREATE OR REPLACE FUNCTION lj_case_name_rank(src text) RETURNS int
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE src
        WHEN 'wikimedia' THEN 3
        WHEN 'ce-list' THEN 2
        WHEN 'source' THEN 1
        WHEN 'renvoi' THEN 0
        ELSE -1
    END
$$;

-- 0346_idx_decisions_wikidata_id.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY decisions_wikidata_id_idx ON decisions (wikidata_id)
WHERE wikidata_id IS NOT NULL;

-- 0347_case_name_source_ranks_first.sql
CREATE OR REPLACE FUNCTION lj_case_name_rank(src text) RETURNS int
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE src
        WHEN 'source' THEN 4
        WHEN 'wikimedia' THEN 3
        WHEN 'ce-list' THEN 2
        WHEN 'renvoi' THEN 0
        ELSE -1
    END
$$;

-- 0348_gazette_tables.sql
CREATE TABLE gazette_dict (
    kind smallint NOT NULL,
    code integer NOT NULL,
    value text NOT NULL,
    PRIMARY KEY (kind, code)
);
CREATE UNIQUE INDEX gazette_dict_value ON gazette_dict (kind, md5(value));

CREATE TABLE gazette_address (
    id integer PRIMARY KEY,
    key bigint NOT NULL,
    shape integer NOT NULL,
    numero text,
    indice text,
    type_voie text,
    nom_voie text,
    compl text,
    localite text,
    bp text,
    cp text,
    ville text,
    pays text,
    etranger text,
    line text,
    rest text
);
CREATE UNIQUE INDEX gazette_address_key ON gazette_address (key);

CREATE TABLE gazette_file (
    source smallint NOT NULL,
    key text NOT NULL,
    checksum bigint NOT NULL,
    notices integer NOT NULL,
    header_shape integer,
    header_rest text,
    ingested_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (source, key)
);

CREATE TABLE bodacc_notice (
    id bigint PRIMARY KEY,
    date date NOT NULL,
    shape integer NOT NULL,
    siren integer,
    address integer,
    address_etab integer,
    origin integer,
    modif integer,
    previous_siren integer,
    jug_date date,
    depot_close date,
    date_activite date,
    date_immat date,
    date_effet date,
    date_cessation date,
    kind smallint NOT NULL,
    famille smallint NOT NULL,
    avis smallint NOT NULL,
    dept smallint,
    tribunal smallint,
    greffe smallint,
    forme smallint,
    jug_famille smallint,
    jug_nature smallint,
    category smallint,
    qualite smallint,
    radiation smallint,
    depot_type smallint,
    depot_desc smallint,
    code_rcs smallint,
    devise smallint,
    name text,
    first_name text,
    usage_name text,
    commercial_name text,
    sigle text,
    enseigne text,
    activity text,
    administration text,
    jug_complement text,
    descriptif text,
    capital text,
    previous_owner text,
    rest text
);
CREATE INDEX bodacc_notice_siren ON bodacc_notice (siren, date) WHERE siren IS NOT NULL;
CREATE INDEX bodacc_notice_previous_siren ON bodacc_notice (previous_siren) WHERE previous_siren IS NOT NULL;

CREATE TABLE bodacc_officer (
    id bigint PRIMARY KEY,
    role integer NOT NULL,
    name text NOT NULL
);

CREATE TABLE joafe_notice (
    id bigint PRIMARY KEY,
    date date NOT NULL,
    shape integer NOT NULL,
    page integer,
    type_code integer,
    theme1 integer,
    theme2 integer,
    theme3 integer,
    siege integer,
    siege_new integer,
    parution_ref integer,
    date_declaration date,
    date_ref date,
    action smallint NOT NULL,
    rectificatif boolean NOT NULL,
    dept smallint,
    type_label smallint,
    theme1_label smallint,
    theme2_label smallint,
    theme3_label smallint,
    lieu smallint,
    rna text,
    titre text,
    objet text,
    cp text,
    fichier text,
    old_title text,
    new_title text,
    email text,
    website text,
    rest text
);
CREATE INDEX joafe_notice_rna ON joafe_notice (rna, date) WHERE rna IS NOT NULL;

-- 0349_decision_text_summary_quote.sql
ALTER TABLE decision_text ADD COLUMN summary_quote_start INTEGER, ADD COLUMN summary_quote_end INTEGER;

-- 0350_legal_text_date_and_paths.sql
ALTER TABLE legal_text ADD COLUMN text_date DATE, ADD COLUMN jurisdiction_paths TEXT[];
ALTER TABLE legal_article ADD COLUMN text_date DATE, ADD COLUMN jurisdiction_paths TEXT[];

-- 0351_legal_article_bm25_v2_text_date_paths.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY legal_article_bm25_v2 ON public.legal_article USING bm25 (id, search_title, texte, num, ((source)::pdb.literal), ((text_uid)::pdb.literal), ((status)::pdb.literal), ((jurisdiction)::pdb.literal), ((nature)::pdb.literal), ((slug)::pdb.literal), ((kind)::pdb.literal), searchable, date_debut, date_fin, text_date, ((jurisdiction_paths)::pdb.literal)) WITH (key_field=id, text_fields='{
    "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "texte":        {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "num":          {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "basic"}
  }');

-- 0352_legal_text_body_bm25_v2_text_date_paths.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY legal_text_body_bm25_v2 ON public.legal_text USING bm25 (id, title, body, ((text_uid)::pdb.literal), ((nature)::pdb.literal), ((jurisdiction)::pdb.literal), ((slug)::pdb.literal), ((role)::pdb.literal), ((COALESCE(status, 'VIGUEUR'::text))::pdb.literal('alias=status_eff')), ((upper(nature))::pdb.literal('alias=nature_up')), ((lower(nature))::pdb.literal('alias=source')), (((body IS NOT NULL))::pdb.alias('alias=has_body')), text_date, ((jurisdiction_paths)::pdb.literal)) WITH (key_field=id, text_fields='{
    "title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "body":  {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}
  }');

-- 0353_legal_text_bm25_swap_text_date_paths.sql
DROP INDEX legal_article_bm25;
DROP INDEX legal_text_body_bm25;

ALTER INDEX legal_article_bm25_v2 RENAME TO legal_article_bm25;
ALTER INDEX legal_text_body_bm25_v2 RENAME TO legal_text_body_bm25;

-- 0354_legal_text_norm_fond_index.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY idx_legal_text_norm_fond ON legal_text (jurisdiction, (jurisdiction_paths[2]), text_date DESC, id) WHERE slug IS NOT NULL;

-- 0355_drop_legal_text_norm_browse_idx.sql
-- lj:sans-transaction
DROP INDEX CONCURRENTLY idx_legal_text_norm_browse;

-- 0356_legal_text_norm_catalogue_index.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY idx_legal_text_norm_catalogue ON legal_text (jurisdiction, (jurisdiction_paths[2]), text_date DESC, id) INCLUDE (jurisdiction_paths) WHERE slug IS NOT NULL;

-- 0357_drop_legal_text_norm_fond_index.sql
-- lj:sans-transaction
DROP INDEX CONCURRENTLY idx_legal_text_norm_fond;

-- 0358_bodacc_notice_bm25.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS bodacc_notice_bm25 ON bodacc_notice
USING bm25 (id, name, first_name, usage_name, commercial_name, sigle, enseigne, activity, administration, descriptif, jug_complement, previous_owner, date, kind, famille, avis, dept)
WITH (
  key_field = 'id',
  text_fields = '{
    "name": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "first_name": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "usage_name": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "commercial_name": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "sigle": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "enseigne": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "activity": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "administration": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "descriptif": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "jug_complement": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "previous_owner": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}
  }'
);

-- 0359_joafe_notice_bm25.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS joafe_notice_bm25 ON joafe_notice
USING bm25 (id, titre, objet, old_title, new_title, date, action, dept)
WITH (
  key_field = 'id',
  text_fields = '{
    "titre": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "objet": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "old_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "new_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}
  }'
);

-- 0360_bodacc_officer_bm25.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS bodacc_officer_bm25 ON bodacc_officer
USING bm25 (id, name, role)
WITH (
  key_field = 'id',
  text_fields = '{
    "name": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}
  }'
);

-- 0361_doctrine_case_names.sql
CREATE TABLE doctrine_deposits (
    hal_id text PRIMARY KEY,
    visited_at timestamptz NOT NULL DEFAULT now(),
    citations jsonb
);

ALTER TABLE decisions DROP CONSTRAINT decisions_case_name_source_check;
ALTER TABLE decisions ADD CONSTRAINT decisions_case_name_source_check
    CHECK (case_name_source IN ('wikimedia', 'ce-list', 'source', 'renvoi', 'doctrine')) NOT VALID;

CREATE OR REPLACE FUNCTION lj_case_name_rank(src text) RETURNS int
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE src
        WHEN 'source' THEN 4
        WHEN 'wikimedia' THEN 3
        WHEN 'ce-list' THEN 2
        WHEN 'doctrine' THEN 1
        WHEN 'renvoi' THEN 0
        ELSE -1
    END
$$;

-- 0362_decision_absorbed_copy.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources ADD COLUMN absorbed BOOLEAN NOT NULL DEFAULT false;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    ADD COLUMN source_rank SMALLINT GENERATED ALWAYS AS (
        CASE
            WHEN absorbed THEN 1
            ELSE CASE source
                WHEN 'judilibre'    THEN 60
                WHEN 'opendata'     THEN 55
                WHEN 'dila-jade'    THEN 50
                WHEN 'dila-constit' THEN 50
                WHEN 'dila-cnil'    THEN 50
                WHEN 'cedh'         THEN 50
                WHEN 'cjue'         THEN 50
                WHEN 'cnda'         THEN 50
                WHEN 'ariane-dce'   THEN 45
                WHEN 'dila-cass'    THEN 30
                WHEN 'dila-inca'    THEN 30
                WHEN 'dila-capp'    THEN 30
                ELSE 0
            END
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

CREATE TABLE decision_redirect (
    public_id   TEXT PRIMARY KEY,
    decision_id BIGINT NOT NULL REFERENCES decisions (id) ON DELETE CASCADE
);

-- 0363_judilibre_rank_reads_dispositif.sql
SET LOCAL lock_timeout = '5s';

DROP TRIGGER decision_sources_mirror_text ON decision_sources;

ALTER TABLE decision_sources
    DROP COLUMN source_rank,
    DROP COLUMN absorbed,
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
            WHEN 'ariane-dce'   THEN 45
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-inca'    THEN 30
            WHEN 'dila-capp'    THEN 30
            ELSE 0
        END
    ) VIRTUAL;

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0364_decision_text_lead_set_once.sql
SET LOCAL lock_timeout = '5s';

CREATE OR REPLACE FUNCTION decision_text_put(p_id BIGINT, p_source_uid TEXT, p_lang TEXT, p_text TEXT) RETURNS VOID LANGUAGE plpgsql AS $$
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
        decision_id, lang, text_status, lead, source_uid, full_text, search_title,
        jurisdiction_type, jurisdiction_code, seat_paths, legal_instruments, legal_article_composite,
        publication_codes, legal_domain_uid, solution_uid, office_uid, procedure_uid, docket_numbers,
        ecli, date_lecture, deleted_at)
    SELECT d.id, p_lang, CASE WHEN p_lang = v_original THEN 'authentic' ELSE 'official' END, FALSE,
           p_source_uid, p_text, d.search_title,
           d.jurisdiction_type, d.jurisdiction_code, d.seat_paths, d.legal_instruments, d.legal_article_composite,
           d.publication_codes, d.legal_domain_uid, d.solution_uid, d.office_uid, d.procedure_uid, d.docket_numbers,
           d.ecli, d.date_lecture, d.deleted_at
      FROM decisions d WHERE d.id = p_id
    ON CONFLICT (decision_id, lang) DO UPDATE SET
        text_status = EXCLUDED.text_status,
        source_uid = EXCLUDED.source_uid,
        full_text = EXCLUDED.full_text
     WHERE (decision_text.text_status, decision_text.source_uid, decision_text.full_text)
           IS DISTINCT FROM (EXCLUDED.text_status, EXCLUDED.source_uid, EXCLUDED.full_text);
    UPDATE decision_text SET lead = TRUE
     WHERE id = (SELECT min(x.id) FROM decision_text x WHERE x.decision_id = p_id)
       AND NOT EXISTS (SELECT 1 FROM decision_text x WHERE x.decision_id = p_id AND x.lead);
END $$;

-- 0365_gazette_notice_counts.sql
CREATE MATERIALIZED VIEW bodacc_notice_count AS
SELECT famille, dept, count(*) AS n FROM bodacc_notice GROUP BY famille, dept;
CREATE UNIQUE INDEX bodacc_notice_count_key ON bodacc_notice_count (famille, dept) NULLS NOT DISTINCT;
CREATE MATERIALIZED VIEW joafe_notice_count AS
SELECT action, dept, count(*) AS n FROM joafe_notice GROUP BY action, dept;
CREATE UNIQUE INDEX joafe_notice_count_key ON joafe_notice_count (action, dept) NULLS NOT DISTINCT;

-- 0366_bodacc_notice_date.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS bodacc_notice_date ON bodacc_notice (date);

-- 0367_joafe_notice_date.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS joafe_notice_date ON joafe_notice (date);

-- 0368_drop_decision_redirect.sql
SET LOCAL lock_timeout = '5s';

DROP TABLE decision_redirect;

-- 0369_summary_batch.sql
CREATE TABLE summary_batch (
    id text PRIMARY KEY,
    engine smallint NOT NULL,
    effort text NOT NULL,
    decision_ids bigint[] NOT NULL,
    submitted_at timestamptz NOT NULL DEFAULT now()
);

-- 0370_disposable_domains.sql
CREATE TABLE disposable_domains (domain text PRIMARY KEY);

-- 0371_drop_target_ref.sql
SET LOCAL lock_timeout = '5s';

ALTER TABLE decision_links DROP COLUMN target_ref;
ALTER TABLE case_citation DROP COLUMN target_ref;
ALTER TABLE text_case_citation DROP COLUMN target_ref;
ALTER TABLE decision_renvoi DROP COLUMN target_ref;

ALTER TABLE decision_links ADD CONSTRAINT decision_links_address_key
    UNIQUE NULLS NOT DISTINCT (decision_id, link_type, target_court, target_docket, target_date);
ALTER TABLE decision_renvoi ADD CONSTRAINT decision_renvoi_address_key
    UNIQUE NULLS NOT DISTINCT (decision_id, renvoi_no, target_court, target_docket, target_date);

-- 0372_decision_submissions.sql
SET LOCAL lock_timeout = '5s';

CREATE TABLE decision_submissions (
    id         BIGSERIAL PRIMARY KEY,
    account    TEXT        NOT NULL REFERENCES users (sub) ON DELETE CASCADE,
    file       BYTEA       NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

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

-- 0373_decision_withdrawal.sql
CREATE TABLE decision_withdrawal (
    public_id         TEXT        PRIMARY KEY,
    jurisdiction_code TEXT        NOT NULL,
    date_lecture      DATE        NOT NULL,
    docket_keys       TEXT[]      NOT NULL,
    reason            TEXT        NOT NULL,
    withdrawn_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE FUNCTION lj_decision_withdrawn(id bigint) RETURNS boolean
LANGUAGE sql STABLE PARALLEL SAFE AS $$
  SELECT EXISTS (
    SELECT 1 FROM decisions d JOIN decision_withdrawal w
      ON w.jurisdiction_code = d.jurisdiction_code AND w.date_lecture = d.date_lecture
     AND w.docket_keys && lj_docket_keys(d.docket_numbers)
     WHERE d.id = $1)
$$;

-- 0374_drop_unread_indexes.sql
SET LOCAL lock_timeout = '5s';

DROP INDEX annonce_titre_trgm_idx;
DROP INDEX annonce_acteur_libelle_trgm_idx;
DROP INDEX assertion_article_idx;
DROP INDEX assertion_places_idx;
DROP INDEX idx_decisions_canonical_ref;
DROP INDEX idx_decisions_docket_numbers;

-- 0375_decision_sources_source_ingested_at.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY idx_decision_sources_source_ingested_at
ON decision_sources (source, ingested_at);

-- 0376_decision_text_summary_stock.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY idx_decision_text_summary_stock
ON decision_text (summary_prompt_version, summary_engine) WHERE lead AND deleted_at IS NULL;

-- 0377_decisions_formation_paths.sql
SET LOCAL lock_timeout = '5s';

ALTER TABLE decisions ADD COLUMN formation_paths TEXT[];
ALTER TABLE decision_text ADD COLUMN formation_paths TEXT[];

CREATE OR REPLACE FUNCTION decision_text_put(p_id BIGINT, p_source_uid TEXT, p_lang TEXT, p_text TEXT) RETURNS VOID LANGUAGE plpgsql AS $$
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
        decision_id, lang, text_status, lead, source_uid, full_text, search_title,
        jurisdiction_type, jurisdiction_code, seat_paths, formation_paths, legal_instruments, legal_article_composite,
        publication_codes, legal_domain_uid, solution_uid, office_uid, procedure_uid, docket_numbers,
        ecli, date_lecture, deleted_at)
    SELECT d.id, p_lang, CASE WHEN p_lang = v_original THEN 'authentic' ELSE 'official' END, FALSE,
           p_source_uid, p_text, d.search_title,
           d.jurisdiction_type, d.jurisdiction_code, d.seat_paths, d.formation_paths, d.legal_instruments, d.legal_article_composite,
           d.publication_codes, d.legal_domain_uid, d.solution_uid, d.office_uid, d.procedure_uid, d.docket_numbers,
           d.ecli, d.date_lecture, d.deleted_at
      FROM decisions d WHERE d.id = p_id
    ON CONFLICT (decision_id, lang) DO UPDATE SET
        text_status = EXCLUDED.text_status,
        source_uid = EXCLUDED.source_uid,
        full_text = EXCLUDED.full_text
     WHERE (decision_text.text_status, decision_text.source_uid, decision_text.full_text)
           IS DISTINCT FROM (EXCLUDED.text_status, EXCLUDED.source_uid, EXCLUDED.full_text);
    UPDATE decision_text SET lead = TRUE
     WHERE id = (SELECT min(x.id) FROM decision_text x WHERE x.decision_id = p_id)
       AND NOT EXISTS (SELECT 1 FROM decision_text x WHERE x.decision_id = p_id AND x.lead);
END $$;

CREATE OR REPLACE FUNCTION decision_text_mirror(p_ids BIGINT[]) RETURNS VOID LANGUAGE sql AS $$
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
        deleted_at = d.deleted_at
      FROM decisions d
     WHERE d.id = ANY(p_ids) AND t.decision_id = d.id
       AND (t.search_title, t.jurisdiction_type, t.jurisdiction_code, t.seat_paths, t.formation_paths, t.legal_instruments,
            t.legal_article_composite, t.publication_codes, t.legal_domain_uid, t.solution_uid, t.office_uid,
            t.procedure_uid, t.docket_numbers, t.ecli, t.date_lecture, t.deleted_at)
           IS DISTINCT FROM
           (d.search_title, d.jurisdiction_type, d.jurisdiction_code, d.seat_paths, d.formation_paths, d.legal_instruments,
            d.legal_article_composite, d.publication_codes, d.legal_domain_uid, d.solution_uid, d.office_uid,
            d.procedure_uid, d.docket_numbers, d.ecli, d.date_lecture, d.deleted_at);
$$;

DROP TRIGGER decisions_mirror_text ON decisions;

CREATE CONSTRAINT TRIGGER decisions_mirror_text
    AFTER INSERT OR UPDATE OF search_title, lang,
        jurisdiction_type, jurisdiction_code, seat_paths, formation_paths, legal_instruments, legal_article_composite,
        publication_codes, legal_domain_uid, solution_uid, office_uid, procedure_uid, docket_numbers,
        ecli, date_lecture, deleted_at
    ON decisions DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0378_decision_text_summary_body.sql
ALTER TABLE decision_text ADD COLUMN summary_body_start INTEGER, ADD COLUMN summary_body_end INTEGER;

-- 0379_decision_text_annex_start.sql
SET LOCAL lock_timeout = '5s';

ALTER TABLE decision_text ADD COLUMN annex_start INTEGER;

DROP FUNCTION decision_text_put(BIGINT, TEXT, TEXT, TEXT);

CREATE FUNCTION decision_text_put(p_id BIGINT, p_source_uid TEXT, p_lang TEXT, p_text TEXT, p_annex_start INTEGER)
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
        decision_id, lang, text_status, lead, source_uid, full_text, annex_start, search_title,
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
    UPDATE decision_text SET lead = TRUE
     WHERE id = (SELECT min(x.id) FROM decision_text x WHERE x.decision_id = p_id)
       AND NOT EXISTS (SELECT 1 FROM decision_text x WHERE x.decision_id = p_id AND x.lead);
END $$;

-- 0380_chunk_zones_embed_summary.sql
SET LOCAL lock_timeout = '5s';

ALTER TABLE decision_chunks ADD COLUMN zones TEXT[] NOT NULL DEFAULT '{text}';
ALTER TABLE decision_chunks ALTER COLUMN zones DROP DEFAULT;

ALTER TABLE decisions ADD COLUMN embed_summary_engine SMALLINT, ADD COLUMN embed_summary_prompt_version SMALLINT;

-- 0381_office_mise_en_etat.sql
INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('office:MISE_EN_ETAT', 'office', 'Juge de la mise en état', 11);

-- 0382_decision_text_bm25_v2.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS decision_text_bm25_v2 ON public.decision_text USING bm25 (id, decision_id, full_text, search_title, ((jurisdiction_type)::pdb.literal), ((legal_instruments)::pdb.literal), ((legal_article_composite)::pdb.literal), ((publication_codes)::pdb.literal), ((jurisdiction_code)::pdb.literal), ((seat_paths)::pdb.literal), ((formation_paths)::pdb.literal), ((legal_domain_uid)::pdb.literal), ((solution_uid)::pdb.literal), ((office_uid)::pdb.literal), ((procedure_uid)::pdb.literal), ((lj_docket_keys(docket_numbers))::pdb.literal('alias=docket_keys')), ((lj_docket_search_keys(docket_numbers))::pdb.literal('alias=docket_search')), ((ecli)::pdb.literal_normalized('alias=ecli')), date_lecture, deleted_at, lead) WITH (key_field=id, text_fields='{"full_text": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}, "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}}');

-- 0383_large_tables_autovacuum_thresholds.sql
ALTER TABLE decision_sources SET (autovacuum_vacuum_scale_factor = 0.02, autovacuum_vacuum_insert_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);
ALTER TABLE decision_actors SET (autovacuum_vacuum_scale_factor = 0.02, autovacuum_vacuum_insert_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);
ALTER TABLE annonce SET (autovacuum_vacuum_scale_factor = 0.02, autovacuum_vacuum_insert_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);

-- 0384_ddd_source_rank.sql
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
    ('jurisdiction_type:DDD', 'jurisdiction_type', 'Défenseur des droits', 'DDD', 15);

-- 0385_raa_document.sql
CREATE TABLE raa_document (
    id bigint PRIMARY KEY,
    url text NOT NULL UNIQUE,
    publisher text NOT NULL,
    dept text,
    published date,
    title text NOT NULL,
    status text NOT NULL DEFAULT 'listed'
        CHECK (status IN ('listed', 'ocr', 'read', 'gone', 'unreadable')),
    listed_at timestamptz NOT NULL DEFAULT now(),
    visited_at timestamptz,
    failures int NOT NULL DEFAULT 0,
    pdf_checksum bigint,
    pages text[],
    ocr_pages int[],
    body text
);

CREATE INDEX raa_document_queue ON raa_document (published DESC) WHERE status IN ('listed', 'ocr');

-- 0386_decision_text_bm25_swap_v2.sql
DROP INDEX decision_text_bm25;

ALTER INDEX decision_text_bm25_v2 RENAME TO decision_text_bm25;

-- 0387_adlc_source_rank.sql
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
    ('jurisdiction_type:ADLC', 'jurisdiction_type', 'Autorité de la concurrence', 'ADLC', 16);

-- 0388_decision_text_part_functions.sql
CREATE OR REPLACE FUNCTION lj_text_before_annexes(full_text TEXT, annex_start INTEGER) RETURNS TEXT
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$ SELECT CASE WHEN annex_start IS NOT NULL THEN left(full_text, annex_start) END $$;
CREATE OR REPLACE FUNCTION lj_text_annexes(full_text TEXT, annex_start INTEGER) RETURNS TEXT
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$ SELECT CASE WHEN annex_start IS NOT NULL THEN substr(full_text, annex_start + 1) END $$;
CREATE OR REPLACE FUNCTION lj_summary_quote(summary TEXT, quote_start INTEGER, quote_end INTEGER) RETURNS TEXT
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$ SELECT CASE WHEN quote_start IS NOT NULL THEN substr(summary, quote_start + 1, quote_end - quote_start) END $$;

-- 0389_decision_text_bm25_parts.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY decision_text_bm25_parts ON public.decision_text USING bm25 (id, decision_id, full_text, search_title, ((jurisdiction_type)::pdb.literal), ((legal_instruments)::pdb.literal), ((legal_article_composite)::pdb.literal), ((publication_codes)::pdb.literal), ((jurisdiction_code)::pdb.literal), ((seat_paths)::pdb.literal), ((formation_paths)::pdb.literal), ((legal_domain_uid)::pdb.literal), ((solution_uid)::pdb.literal), ((lj_docket_keys(docket_numbers))::pdb.literal('alias=docket_keys')), ((lj_docket_search_keys(docket_numbers))::pdb.literal('alias=docket_search')), ((ecli)::pdb.literal_normalized('alias=ecli')), date_lecture, deleted_at, lead, annex_start, ((lj_text_before_annexes(full_text, annex_start))::pdb.regex_pattern('\p{N}+(?:[./-]\p{N}+)+|[\p{L}\p{N}-]+', 'ascii_folding=true', 'stopwords_language=French', 'alias=text_before_annexes')), ((lj_text_annexes(full_text, annex_start))::pdb.regex_pattern('\p{N}+(?:[./-]\p{N}+)+|[\p{L}\p{N}-]+', 'ascii_folding=true', 'stopwords_language=French', 'alias=annexes')), ((lj_summary_quote(summary, summary_quote_start, summary_quote_end))::pdb.regex_pattern('\p{N}+(?:[./-]\p{N}+)+|[\p{L}\p{N}-]+', 'ascii_folding=true', 'stopwords_language=French', 'alias=official_summary'))) WITH (key_field=id, text_fields='{"full_text": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}, "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}}');

-- 0390_decision_text_bm25_parts_swap.sql
DO $$ BEGIN IF NOT (SELECT indisvalid FROM pg_index WHERE indexrelid = 'decision_text_bm25_parts'::regclass) THEN RAISE EXCEPTION 'decision_text_bm25_parts is not valid'; END IF; END $$;
DROP INDEX decision_text_bm25;
ALTER INDEX decision_text_bm25_parts RENAME TO decision_text_bm25;

-- 0391_decision_text_headnote_index.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY decision_text_headnote ON public.decision_text (decision_id) WHERE summary_quote_start IS NOT NULL;

-- 0392_decision_chunks_headnote_tag.sql
CREATE OR REPLACE FUNCTION lj_chunk_headnote() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  NEW.zones := array_remove(NEW.zones, 'headnote')
    || CASE WHEN EXISTS (SELECT 1 FROM decision_text t WHERE t.decision_id = NEW.decision_id AND t.summary_quote_start IS NOT NULL)
       THEN ARRAY['headnote'] ELSE ARRAY[]::text[] END;
  RETURN NEW;
END $$;
CREATE TRIGGER decision_chunks_headnote BEFORE INSERT OR UPDATE OF zones ON decision_chunks
  FOR EACH ROW EXECUTE FUNCTION lj_chunk_headnote();
CREATE OR REPLACE FUNCTION lj_text_headnote() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE d bigint := CASE WHEN TG_OP = 'DELETE' THEN OLD.decision_id ELSE NEW.decision_id END;
BEGIN
  UPDATE decision_chunks SET zones = zones WHERE decision_id = d
    AND ('headnote' = ANY(zones)) IS DISTINCT FROM EXISTS (SELECT 1 FROM decision_text t WHERE t.decision_id = d AND t.summary_quote_start IS NOT NULL);
  RETURN NULL;
END $$;
CREATE TRIGGER decision_text_headnote AFTER INSERT OR DELETE OR UPDATE OF summary_quote_start ON decision_text
  FOR EACH ROW EXECUTE FUNCTION lj_text_headnote();

-- 0393_text_publication_outside_norms.sql
ALTER TABLE text_publication
    ADD COLUMN journal_code TEXT NOT NULL DEFAULT 'JORF',
    ADD COLUMN nature TEXT,
    ADD COLUMN nor TEXT,
    ADD COLUMN date_texte DATE,
    ADD COLUMN url TEXT,
    ADD COLUMN body TEXT;

-- 0394_comeu_source_rank.sql
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
    ('jurisdiction_type:COMEU', 'jurisdiction_type', 'Commission européenne', 'COMEU', 17);

-- 0395_foreign_court_places_and_types.sql
INSERT INTO facet_value (uid, facet, label, abbr, parent_uid, sort) VALUES
    ('geo:OHADA',  'geo', 'Organisation pour l''harmonisation en Afrique du droit des affaires', 'OHADA', NULL, 174),
    ('geo:UEMOA',  'geo', 'Union économique et monétaire ouest-africaine', 'UEMOA', NULL, 175),
    ('geo:CEMAC',  'geo', 'Communauté économique et monétaire de l''Afrique centrale', 'CEMAC', NULL, 176),
    ('geo:CEDEAO', 'geo', 'Communauté économique des États de l''Afrique de l''Ouest', 'CEDEAO', NULL, 177);

INSERT INTO facet_value (uid, facet, label, abbr, sort) VALUES
    ('jurisdiction_type:COUR_SUPREME',               'jurisdiction_type', 'Cour suprême', NULL, 18),
    ('jurisdiction_type:COUR_CASSATION',             'jurisdiction_type', 'Cour de cassation', NULL, 19),
    ('jurisdiction_type:COUR_CONSTITUTIONNELLE',     'jurisdiction_type', 'Cour constitutionnelle', NULL, 20),
    ('jurisdiction_type:COUR_APPEL',                 'jurisdiction_type', 'Cour d''appel', NULL, 21),
    ('jurisdiction_type:COUR_ADMINISTRATIVE',        'jurisdiction_type', 'Cour administrative', NULL, 22),
    ('jurisdiction_type:TRIBUNAL_ADMINISTRATIF',     'jurisdiction_type', 'Tribunal administratif', NULL, 23),
    ('jurisdiction_type:TRIBUNAL_PREMIERE_INSTANCE', 'jurisdiction_type', 'Tribunal de première instance', NULL, 24),
    ('jurisdiction_type:TRIBUNAL_COMMERCE',          'jurisdiction_type', 'Tribunal de commerce', NULL, 25),
    ('jurisdiction_type:COUR_CRIMINELLE',            'jurisdiction_type', 'Cour criminelle', NULL, 26),
    ('jurisdiction_type:COUR_JUSTICE',               'jurisdiction_type', 'Cour de justice', NULL, 27),
    ('jurisdiction_type:CCJA',                       'jurisdiction_type', 'Cour commune de justice et d''arbitrage', 'CCJA', 28),
    ('jurisdiction_type:COUR_DROITS_HOMME',          'jurisdiction_type', 'Cour des droits de l''homme', NULL, 29);

