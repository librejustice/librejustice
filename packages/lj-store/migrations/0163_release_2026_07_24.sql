-- 0119_search_title_drop_expression.sql
ALTER TABLE decisions ALTER COLUMN search_title DROP EXPRESSION;

-- 0120_drop_legacy_title_columns.sql
SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

CREATE OR REPLACE FUNCTION sync_chunks_decision_meta()
RETURNS trigger LANGUAGE plpgsql AS $fn$
BEGIN
    UPDATE decision_chunks SET
        publication_codes  = NEW.publication_codes,
        date_lecture       = NEW.date_lecture,
        search_title       = CASE WHEN chunk_index = 0 THEN NEW.search_title END
    WHERE decision_id = NEW.id;
    RETURN NULL;
END;
$fn$;

DROP TRIGGER IF EXISTS decisions_sync_chunks_meta ON decisions;
CREATE TRIGGER decisions_sync_chunks_meta
AFTER UPDATE ON decisions
FOR EACH ROW WHEN (
       OLD.publication_codes IS DISTINCT FROM NEW.publication_codes
    OR OLD.date_lecture      IS DISTINCT FROM NEW.date_lecture
    OR OLD.search_title      IS DISTINCT FROM NEW.search_title
)
EXECUTE FUNCTION sync_chunks_decision_meta();

DROP INDEX IF EXISTS idx_chunks_jurisdiction_name;
ALTER TABLE decision_chunks DROP COLUMN IF EXISTS jurisdiction_name;

DO $do$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_indexes
        WHERE indexname = 'decisions_bm25' AND indexdef LIKE '%jurisdiction_name%'
    ) THEN
        EXECUTE 'DROP INDEX decisions_bm25';
        EXECUTE $ix$
CREATE INDEX decisions_bm25 ON decisions USING bm25 (id, full_text, search_title, ((juridiction_type)::pdb.literal), ((legal_instruments)::pdb.literal), ((legal_article_composite)::pdb.literal), ((publication_codes)::pdb.literal), date_lecture) WITH (key_field=id, text_fields='{
    "full_text":    {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"},
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}
  }')
        $ix$;
    END IF;
END;
$do$;

ALTER TABLE decisions
    DROP COLUMN IF EXISTS jurisdiction_name,
    DROP COLUMN IF EXISTS formation_or_chamber;

ANALYZE decisions;
ANALYZE decision_chunks;

-- 0121_jurisdiction_cassation_court_grain.sql
UPDATE decisions SET jurisdiction_code = 'cc'
WHERE juridiction_type = 'CC' AND jurisdiction_code <> 'cc';

DELETE FROM jurisdiction WHERE code LIKE 'cass\_%';

ANALYZE decisions;

-- 0122_legal_link_graph.sql
CREATE TABLE legal_link (
    owner_text_uid   TEXT NOT NULL,
    owner_num_key    TEXT NOT NULL,
    owner_date_debut DATE NOT NULL,
    seq              INTEGER NOT NULL,

    typelien         TEXT NOT NULL,
    verb             TEXT NOT NULL,
    direction        TEXT NOT NULL,

    target_kind      TEXT NOT NULL,
    target_uid       TEXT,
    target_text_uid  TEXT,
    target_num       TEXT,
    target_num_key   TEXT,
    target_nature    TEXT,
    target_label     TEXT NOT NULL,
    target_date      DATE,
    target_nor       TEXT,
    PRIMARY KEY (owner_text_uid, owner_num_key, owner_date_debut, seq)
);

CREATE INDEX idx_legal_link_target_uid ON legal_link (target_uid)
    WHERE target_uid IS NOT NULL;
CREATE INDEX idx_legal_link_target_art ON legal_link (target_text_uid, target_num_key)
    WHERE target_text_uid IS NOT NULL;

CREATE INDEX idx_legal_article_source_uid ON legal_article (source_uid);

-- 0123_legal_toc_edge.sql
CREATE TABLE legal_toc_edge (
    owner_uid     TEXT NOT NULL,
    text_uid      TEXT NOT NULL,
    seq           INTEGER NOT NULL,
    child_kind    TEXT NOT NULL,
    child_uid     TEXT NOT NULL,
    child_cid     TEXT,
    child_num_key TEXT,
    label         TEXT NOT NULL,
    etat          TEXT NOT NULL,
    date_debut    DATE,
    date_fin      DATE,
    niv           INTEGER,
    PRIMARY KEY (owner_uid, seq)
);

CREATE INDEX idx_legal_toc_edge_text ON legal_toc_edge (text_uid);

CREATE INDEX idx_legal_toc_edge_child_uid ON legal_toc_edge (child_uid);
CREATE INDEX idx_legal_toc_edge_child_cid ON legal_toc_edge (child_cid)
    WHERE child_cid IS NOT NULL;

-- 0124_legal_text_upcoming_versions.sql
ALTER TABLE legal_text
    ADD COLUMN upcoming_versions date[] NOT NULL DEFAULT '{}';

-- 0125_entity_denomination_tables.sql
CREATE TABLE entity (
    uid                 text PRIMARY KEY,
    nature              text NOT NULL CHECK (nature IN
                            ('morale_privee', 'morale_publique', 'physique')),
    denomination        text NOT NULL,
    denomination_folded text NOT NULL,
    sigle               text,
    forme               text,
    active              bool NOT NULL,
    updated_at          timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX entity_denomination_folded_idx ON entity (denomination_folded);
ALTER TABLE entity SET (fillfactor = 100,
    autovacuum_vacuum_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);

CREATE TABLE entity_denomination (
    entity_uid   text NOT NULL REFERENCES entity(uid) ON DELETE CASCADE,
    denomination text NOT NULL,
    folded       text NOT NULL,
    date_debut   date,
    date_fin     date
);
CREATE INDEX entity_denomination_uid_idx ON entity_denomination (entity_uid);
CREATE INDEX entity_denomination_resolve_idx ON entity_denomination (folded);
ALTER TABLE entity_denomination SET (fillfactor = 100,
    autovacuum_vacuum_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);

-- 0126_decisions_intervenors.sql
ALTER TABLE decisions ADD COLUMN intervenors text[] NOT NULL DEFAULT '{}';

-- 0127_decision_party.sql
CREATE TABLE decision_party (
    decision_id  bigint NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    ord          int    NOT NULL,
    quality      text   NOT NULL CHECK (quality IN
                     ('party', 'law_firm', 'counsel_name', 'intervenor')),
    side         text   CHECK (side IN ('applicant', 'defendant')),
    value        text   NOT NULL,
    resolve_key  text   NOT NULL,
    entity_uid   text,
    PRIMARY KEY (decision_id, ord)
);

CREATE INDEX decision_party_pending_idx ON decision_party (resolve_key)
    WHERE entity_uid IS NULL AND quality IN ('party', 'intervenor', 'law_firm');

CREATE INDEX decision_party_entity_idx ON decision_party (entity_uid)
    WHERE entity_uid IS NOT NULL;

-- 0128_drop_office_juge_referes.sql
DELETE FROM facet_value WHERE uid = 'office:JUGE_REFERES';

-- 0129_juridiction_cnil_source_rank.sql
INSERT INTO facet_value (uid, facet, label, abbr, sort) VALUES
    ('juridiction:CNIL', 'juridiction', 'Commission nationale de l''informatique et des libertés', 'CNIL', 15);

ALTER TABLE decision_sources DROP COLUMN source_rank;

ALTER TABLE decision_sources
    ADD COLUMN source_rank SMALLINT NOT NULL GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN 60
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-capp'    THEN 30
            ELSE 0
        END
    ) STORED;

-- 0130_decision_party_evidence_columns.sql
ALTER TABLE decision_party
    ADD COLUMN char_starts int4[],
    ADD COLUMN char_ends int4[],
    ADD COLUMN nature text CHECK (nature IN
        ('physique', 'morale_privee', 'morale_publique')),
    ADD COLUMN extract_version int2 NOT NULL DEFAULT 0;
ALTER TABLE decision_party ALTER COLUMN extract_version DROP DEFAULT;

DROP INDEX decision_party_pending_idx;
CREATE INDEX decision_party_pending_idx ON decision_party (resolve_key)
    WHERE entity_uid IS NULL AND quality IN ('party', 'law_firm');

-- 0131_decision_party_barreau.sql
ALTER TABLE decision_party ADD COLUMN barreau text;

DROP INDEX decision_party_pending_idx;
CREATE INDEX decision_party_pending_idx ON decision_party (resolve_key)
    WHERE entity_uid IS NULL
      AND quality IN ('party', 'law_firm', 'counsel_name');

-- 0132_entity_contentieux.sql
CREATE TABLE entity_contentieux (
    entity_uid          text PRIMARY KEY REFERENCES entity(uid) ON DELETE CASCADE,

    category            text NOT NULL,
    namespace           text NOT NULL,
    nature              text NOT NULL,
    denomination        text NOT NULL,
    denomination_folded text NOT NULL,
    forme               text,
    active              bool NOT NULL,

    barreau             text,
    decision_count      bigint NOT NULL
);

CREATE INDEX entity_contentieux_listing_idx
    ON entity_contentieux (category, decision_count DESC, denomination_folded);

CREATE INDEX entity_contentieux_barreau_idx
    ON entity_contentieux (barreau, decision_count DESC, denomination_folded)
    WHERE barreau IS NOT NULL;

CREATE INDEX entity_contentieux_search_idx
    ON entity_contentieux (denomination_folded text_pattern_ops);

ALTER TABLE entity_contentieux SET (fillfactor = 100,
    autovacuum_vacuum_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);

-- 0133_decision_party_role.sql
ALTER TABLE decision_party ADD COLUMN role text
    CHECK (role IN ('substituant', 'substitue', 'postulant', 'plaidant'));

-- 0134_entity_surname_key.sql
ALTER TABLE entity ADD COLUMN surname_key text;
CREATE INDEX entity_surname_key_idx ON entity (surname_key)
    WHERE surname_key IS NOT NULL;

-- 0135_registry_announcements.sql
CREATE TABLE registre_annonce (
    id             BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,

    source_uid     TEXT NOT NULL UNIQUE,

    source_file    TEXT NOT NULL,
    registre       TEXT NOT NULL,
    famille        TEXT NOT NULL,
    type_avis      TEXT NOT NULL,
    entity_uids    TEXT[] NOT NULL DEFAULT '{}',
    denomination   TEXT,
    tribunal       TEXT,
    date_evenement DATE,
    date_parution  DATE NOT NULL,
    parution       TEXT,
    numero_annonce INTEGER,
    details        JSONB NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX registre_annonce_entity_uids_idx ON registre_annonce USING gin (entity_uids);

CREATE INDEX registre_annonce_famille_idx ON registre_annonce (registre, famille);

CREATE INDEX registre_annonce_source_file_idx ON registre_annonce (source_file);

CREATE TABLE registre_annonce_file (
    file_name   TEXT PRIMARY KEY,
    checksum    BIGINT NOT NULL,
    annonces    INTEGER NOT NULL,
    ingested_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 0136_legal_text_body_case_citation.sql
ALTER TABLE legal_text ADD COLUMN body text;

ALTER TABLE legal_text ADD COLUMN status text;

DROP INDEX referential_texts_title_bm25;

CREATE INDEX legal_text_body_bm25 ON legal_text
USING bm25 (
    id,
    title,
    body,
    (nature::pdb.literal),
    (jurisdiction::pdb.literal)
)
WITH (
  key_field = 'id',
  text_fields = '{
    "title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "body":  {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}
  }'
);

CREATE TABLE text_case_citation (
    owner_text_uid     text NOT NULL REFERENCES legal_text(text_uid) ON DELETE CASCADE,
    owner_num_key      text,
    owner_date_debut   date,
    char_start         int4 NOT NULL,
    char_end           int4 NOT NULL,
    target_ref         text NOT NULL,
    target_decision_id int8 REFERENCES decisions(id) ON DELETE SET NULL,
    extract_version    int2 NOT NULL,
    CHECK (char_end > char_start),
    CHECK ((owner_num_key IS NULL) = (owner_date_debut IS NULL))
);

CREATE UNIQUE INDEX text_case_citation_owner_span_key
    ON text_case_citation (owner_text_uid, COALESCE(owner_num_key, ''),
                           COALESCE(owner_date_debut, '0001-01-01'::date), char_start);

CREATE INDEX text_case_citation_pending_target_idx
    ON text_case_citation (target_ref) WHERE target_decision_id IS NULL;

CREATE INDEX text_case_citation_target_decision_idx
    ON text_case_citation (target_decision_id) WHERE target_decision_id IS NOT NULL;

-- 0137_jurisdiction_code_by_city.sql
ALTER TABLE jurisdiction ADD COLUMN source_code TEXT;
UPDATE jurisdiction SET source_code = code;
ALTER TABLE jurisdiction ALTER COLUMN source_code SET NOT NULL;
ALTER TABLE jurisdiction ADD CONSTRAINT jurisdiction_source_code_key UNIQUE (source_code);

ALTER TABLE decisions DROP CONSTRAINT decisions_jurisdiction_code_fkey;

UPDATE jurisdiction
SET code = lower(juridiction_type) || '_' || trim(both '_' from regexp_replace(
        translate(lower(replace(replace(city, 'œ', 'oe'), 'Œ', 'oe')),
                  'àâäáéèêëíîïóôöúùûüçÿñ', 'aaaaeeeeiiioooouuuucyn'),
        '[^a-z0-9]+', '_', 'g'))
WHERE juridiction_type IN ('TJ', 'TCOM') AND city IS NOT NULL;

UPDATE decisions d
SET jurisdiction_code = j.code
FROM jurisdiction j
WHERE d.jurisdiction_code = j.source_code
  AND j.code <> j.source_code;

ALTER TABLE decisions ADD CONSTRAINT decisions_jurisdiction_code_fkey
    FOREIGN KEY (jurisdiction_code) REFERENCES jurisdiction(code);

UPDATE user_search_history h
SET filters = jsonb_set(h.filters, '{jurisdictionCode}', (
        SELECT jsonb_agg(coalesce(j.code, x.v))
        FROM jsonb_array_elements_text(h.filters -> 'jurisdictionCode') AS x(v)
        LEFT JOIN jurisdiction j ON j.source_code = x.v AND j.code <> j.source_code))
WHERE h.filters ? 'jurisdictionCode';

-- 0138_jurisdiction_referential_repair.sql
UPDATE decisions SET jurisdiction_code = 'ta_amiens' WHERE jurisdiction_code = 'ta_d_amiens';
UPDATE decisions SET jurisdiction_code = 'caa_versailles' WHERE jurisdiction_code = 'caa_versailless';
UPDATE decisions SET jurisdiction_code = 'caa_marseille' WHERE jurisdiction_code = 'caa_montpellier';
DELETE FROM jurisdiction WHERE code IN ('ta_d_amiens', 'caa_versailless', 'caa_montpellier');

UPDATE jurisdiction j
SET city = v.city
FROM (VALUES
    ('tj_havre', 'Le Havre'),
    ('tcom_havre', 'Le Havre'),
    ('tj_mans', 'Le Mans'),
    ('tcom_mans', 'Le Mans'),
    ('tcom_puy_en_velay', 'Le Puy-en-Velay'),
    ('tcom_roche_sur_yon', 'La Roche-sur-Yon'),
    ('tj_sables_d_olonne', 'Les Sables-d''Olonne'),
    ('ta_reunion', 'La Réunion'),
    ('ta_st_barthelemy', 'Saint-Barthélemy'),
    ('ta_st_martin', 'Saint-Martin')
) AS v(code, city)
WHERE j.code = v.code;

UPDATE jurisdiction SET label = 'Tribunal administratif de Saint-Barthélemy' WHERE code = 'ta_st_barthelemy';
UPDATE jurisdiction SET label = 'Tribunal administratif de Saint-Martin' WHERE code = 'ta_st_martin';

CREATE TEMP TABLE jurisdiction_rename AS
SELECT code AS old_code,
       lower(juridiction_type) || '_' || trim(both '_' from regexp_replace(
           translate(lower(replace(replace(city, 'œ', 'oe'), 'Œ', 'oe')),
                     'àâäáéèêëíîïóôöúùûüçÿñ', 'aaaaeeeeiiiooouuuucyn'),
           '[^a-z0-9]+', '_', 'g')) AS new_code
FROM jurisdiction
WHERE juridiction_type IN ('TJ', 'TCOM', 'CA', 'CAA', 'TA') AND city IS NOT NULL;
DELETE FROM jurisdiction_rename WHERE new_code = old_code;

ALTER TABLE decisions DROP CONSTRAINT decisions_jurisdiction_code_fkey;

UPDATE jurisdiction j SET code = r.new_code
FROM jurisdiction_rename r WHERE j.code = r.old_code;

UPDATE decisions d SET jurisdiction_code = r.new_code
FROM jurisdiction_rename r WHERE d.jurisdiction_code = r.old_code;

ALTER TABLE decisions ADD CONSTRAINT decisions_jurisdiction_code_fkey
    FOREIGN KEY (jurisdiction_code) REFERENCES jurisdiction(code);

UPDATE user_search_history h
SET filters = jsonb_set(h.filters, '{jurisdictionCode}', (
        SELECT jsonb_agg(coalesce(m.new_code, x.v))
        FROM jsonb_array_elements_text(h.filters -> 'jurisdictionCode') AS x(v)
        LEFT JOIN (
            SELECT old_code, new_code FROM jurisdiction_rename
            UNION ALL
            VALUES ('ta_d_amiens', 'ta_amiens'),
                   ('caa_versailless', 'caa_versailles'),
                   ('caa_montpellier', 'caa_marseille')
        ) m ON m.old_code = x.v))
WHERE h.filters ? 'jurisdictionCode';

DROP TABLE jurisdiction_rename;

-- 0139_drop_registry_announcements.sql
DROP TABLE IF EXISTS registre_annonce;
DROP TABLE IF EXISTS registre_annonce_file;

-- 0140_pg_trgm_extension.sql
CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- 0141_legal_text_slug_cleanup.sql
WITH renames(text_uid, new_slug) AS (VALUES
  ('jafbase/docAsie/Armenie/CodeCivil.rtf','code-civil-arm'),
  ('jafbase/docAfrique/Angola/filiation angola.pdf','code-famille-filiation-ago'),
  ('jafbase/docAfrique/Angola/mariage.pdf','loi-acte-mariage-11-85-ago'),
  ('jafbase/docAfrique/Angola/divorce.pdf','loi-divorce-9-78-ago'),
  ('translated/code-autrichien-de-procedure-penale','code-de-procedure-penale-aut'),
  ('translated/constitution-autrichienne','constitution-aut'),
  ('belgian/code-civil-belge','code-civil-bel'),
  ('belgian/code-civil-belge-nouveau','code-civil-nouveau-code-civil-bel'),
  ('belgian/code-de-droit-economique','code-de-droit-economique-bel'),
  ('belgian/code-des-societes-belge','code-des-societes-bel'),
  ('belgian/code-instruction-criminelle-belge','code-d-instruction-criminelle-bel'),
  ('belgian/code-judiciaire','code-judiciaire-bel'),
  ('belgian/code-penal-belge','code-penal-bel'),
  ('mjp.univ-perp.fr/be1994','constitution-1994-bel'),
  ('african/code-de-procedure-penale-burkinabe-bf','code-de-procedure-penale-bfa'),
  ('african/code-des-personnes-et-de-la-famille-burkina-bf','code-des-personnes-et-de-la-famille-bfa'),
  ('african/code-procedure-civile-burkinabe-1999-bf','code-procedure-civile-1999-bfa'),
  ('translated/code-penal-bulgare-bg','code-penal-bgr'),
  ('mjp.univ-perp.fr/bg1991','constitution-1991-bgr'),
  ('jafbase/docAfrique/Burundi/Burundi-Code-civil.pdf','code-civil-bdi'),
  ('african/code-personnes-famille-burundi-bi','code-personnes-famille-bdi'),
  ('african/code-de-la-nationalite-beninoise-bj','code-de-la-nationalite-ben'),
  ('jafbase/docAfrique/Benin/Benin_Family_Code.pdf','code-des-personnes-et-de-la-famille-loi-n-2002-07-ben'),
  ('african/code-procedure-civile-commerciale-sociale-benin-bj','code-procedure-civile-commerciale-sociale-ben'),
  ('african/code-procedure-penale-beninois-2012-15-bj','code-procedure-penale-2012-15-ben'),
  ('african/code-enfant-benin-2015','code-enfant-ben'),
  ('african/loi-2020-34-etat-civil-beninoise-bj','loi-2020-34-etat-civil-ben'),
  ('jafbase/docAfrique/Congos/RDC Congo code famille 1987.pdf','code-de-la-famille-loi-n87-010-du-1er-aout-1987-cod'),
  ('african/code-procedure-civile-congolais-rdc-cd','code-procedure-civile-cod'),
  ('african/code-procedure-penale-congolais-rdc-cd','code-procedure-penale-cod'),
  ('african/loi-nationalite-congolaise-rdc-cd','loi-nationalite-cod'),
  ('jafbase/docAfrique/Centrafrique/code de la famille.pdf','code-de-la-famille-loi-n-97-013-caf'),
  ('jafbase/docAfrique/Centrafrique/Code de procedure civile  Centrafrique.pdf','code-procedure-civile-caf'),
  ('jafbase/docAfrique/Congos/Brazzaville code famille 1984.pdf','code-de-la-famille-loi-n073-84-du-17-octobre-1984-cog'),
  ('african/code-de-la-nationalite-congolaise-cg','code-de-la-nationalite-cog'),
  ('african/code-procedure-civile-commerciale-administrative-financiere-congo-cg','code-procedure-civile-commerciale-administrative-financiere-cog'),
  ('african/code-procedure-penale-congolais-brazzaville-cg','code-procedure-penale-cog'),
  ('fedlex/eli/cc/24/233_245_233','code-civil-che'),
  ('fedlex/eli/cc/27/317_321_377','code-des-obligations-che'),
  ('fedlex/eli/cc/1999/404','constitution-federale-che'),
  ('fedlex/eli/cc/1988/1776_1776_1776','loi-droit-international-prive-ldip-che'),
  ('fedlex/eli/cc/2002/510','loi-partie-generale-assurances-sociales-lpga-che'),
  ('fedlex/eli/cc/11/529_488_529','loi-poursuite-dettes-faillite-lp-che'),
  ('african/code-civil-ci-biens','code-civil-biens-civ'),
  ('african/code-civil-ci-contrats-speciaux','code-civil-contrats-speciaux-civ'),
  ('african/code-civil-ci-majeurs-proteges','code-civil-majeurs-proteges-civ'),
  ('african/code-civil-ci-obligations','code-civil-obligations-civ'),
  ('african/code-civil-ci-suretes-prescription','code-civil-suretes-prescription-civ'),
  ('african/code-de-la-nationalite-ivoirienne-ci','code-de-la-nationalite-civ'),
  ('african/code-procedure-civile-commerciale-administrative-ci','code-procedure-civile-commerciale-administrative-civ'),
  ('african/code-procedure-penale-ivoirien-2018-975-ci','code-procedure-penale-2018-975-civ'),
  ('african/loi-2018-862-etat-civil-ci','loi-2018-862-etat-civil-civ'),
  ('african/loi-2018-863-declaration-naissance-deces-ci','loi-2018-863-declaration-naissance-deces-civ'),
  ('african/loi-2019-570-mariage-ivoirienne-ci','loi-2019-570-mariage-civ'),
  ('african/loi-2019-571-filiation-ci','loi-2019-571-filiation-civ'),
  ('african/loi-2020-490-nom-ci','loi-2020-490-nom-civ'),
  ('african/loi-adoption-ci','loi-adoption-civ'),
  ('jafbase/docAfrique/CotedIvoire/Loi_ivoirienne_du_26juin2019_relative_à_la_minorité.pdf','loi-n-2019-572-du-26-juin-2019-relative-a-la-minorite-civ'),
  ('jafbase/docAfrique/CotedIvoire/Cote-dIvoire-état civil.pdf','loi-n-64-374-du-7-octobre-1964-relative-a-l-etat-civil-civ'),
  ('african/loi-paternite-filiation-ivoirienne-64-377-ci','loi-paternite-filiation-64-377-civ'),
  ('jafbase/docAfrique/Cameroun/CODE-CIVIL-CAMEROUNAIS complet.pdf','code-civil-cmr'),
  ('african/code-de-la-nationalite-camerounaise-cm','code-de-la-nationalite-cmr'),
  ('african/code-de-procedure-penale-camerounais-cm','code-de-procedure-penale-cmr'),
  ('faolex.fao.org/CMR-code-travail-92-007','code-du-travail-cmr'),
  ('african/code-penal-camerounais-cm','code-penal-cmr'),
  ('african/code-procedure-civile-commerciale-camerounais-cm','code-procedure-civile-commerciale-cmr'),
  ('cvuc-uccc.com/CMR-ordonnance-81-02-etat-civil','ordonnance-81-02-etat-civil-cmr'),
  ('gesetze-im-internet.de/bgb','code-civil-bgb-deu'),
  ('gesetze-im-internet.de/hgb','code-commerce-hgb-deu'),
  ('gesetze-im-internet.de/stgb','code-penal-stgb-deu'),
  ('gesetze-im-internet.de/gg','loi-fondamentale-gg-deu'),
  ('jafbase/docAfrique/Djibouti/CodeFamille.pdf','code-de-la-famille-dji'),
  ('african/code-de-la-nationalite-djiboutienne-dj','code-de-la-nationalite-dji'),
  ('jafbase/docAfrique/Djibouti/Code de procedure civile 2018 DJIBOUTI.pdf','code-procedure-civile-2018-dji'),
  ('african/code-civil-djibouti-2018','code-civil-dji'),
  ('jafbase/DocAmeriques/Dominicaine/LoiDivorce.pdf','loi-divorce-1306-dom'),
  ('clr.africanchildforum.org/DZA-code-civil-75-58','code-civil-dza'),
  ('jafbase/docMaghreb/Algeriecode_famille.pdf','code-de-la-famille-dza'),
  ('african/code-de-la-nationalite-algerienne-dz','code-de-la-nationalite-dza'),
  ('who.int/cpcd/DZA-ordonnance-70-20-etat-civil','code-de-l-etat-civil-dza'),
  ('jafbase/docMaghreb/algerie code procedure.pdf','code-de-procedure-civile-et-administrative-dza'),
  ('policehumanrightsresources.org/DZA-code-penal-1966','code-penal-1966-dza'),
  ('jafbase/docMaghreb/AlgerieConvEnfantsMixtes.pdf','convention-franco-algerienne-enfants-couples-mixtes-1988-dza'),
  ('foreign/code-civil-egyptien-eg','code-civil-egy'),
  ('foreign/code-commerce-egyptien-eg','code-commerce-egy'),
  ('foreign/code-penal-egyptien-eg','code-penal-egy'),
  ('foreign/code-procedure-penale-egyptien-eg','code-procedure-penale-egy'),
  ('jafbase/docAfrique/Egypte/traduction loi 100 de 1985.pdf','loi-100-1985-statut-personnel-egy'),
  ('jafbase/docAfrique/Egypte/traduction loi 44 de 1979 remplacée par la loi 100 de 1985.pdf','loi-44-1979-statut-personnel-egy'),
  ('jafbase/docAfrique/Egypte/traduction lois de 1920 et 1929.pdf','lois-statut-personnel-1920-1929-egy'),
  ('boe.es/codigo-civil','code-civil-esp'),
  ('translated/code-de-procedure-civile-espagnol-es','code-de-procedure-civile-esp'),
  ('translated/code-penal-espagnol-es','code-penal-esp'),
  ('mjp.univ-perp.fr/es1978','constitution-1978-esp'),
  ('faolex.fao.org/GAB-code-civil-15-72-p1','code-civil-loi-15-72-p1-gab'),
  ('faolex.fao.org/GAB-code-civil-p2-1989','code-civil-p2-successions-gab'),
  ('african/code-de-la-nationalite-gabonaise-ga','code-de-la-nationalite-gab'),
  ('african/code-de-procedure-civile-gabonais-ga','code-de-procedure-civile-gab'),
  ('african/code-procedure-penale-gabonais-ga','code-procedure-penale-gab'),
  ('jafbase/docAfrique/Guinee/Code-civil-de-2019.pdf','code-civil-gin'),
  ('african/code-de-l-enfant-guineen-gn','code-de-l-enfant-gin'),
  ('a-mla.org/GIN-CPCEA-1998-D98-100','code-de-procedure-civile-economique-et-administrative-gin'),
  ('african/code-procedure-penale-guineen-2016-gn','code-procedure-penale-2016-gin'),
  ('translated/code-civil-grec-gr','code-civil-grc'),
  ('translated/code-des-douanes-grec','code-des-douanes-grc'),
  ('mjp.univ-perp.fr/gr2019','constitution-2019-grc'),
  ('foreign/code-civil-haitien-ht','code-civil-hti'),
  ('jafbase/DocAmeriques/Haiti/loi-sur-la-paternite-et-la-filiation- 2014 pdf.pdf','loi-paternite-maternite-filiation-hti'),
  ('jafbase/DocAmeriques/Haiti/loi-reformant-l-adoption-en-haiti_haitijustice.pdf','loi-reformant-l-adoption-hti'),
  ('translated/code-civil-hongrois-hu','code-civil-hun'),
  ('translated/code-de-procedure-penale-hongrois','code-de-procedure-penale-hun'),
  ('translated/code-du-travail-hongrois','code-du-travail-hun'),
  ('jafbase/docAsie/Irak/code civil irakien 1951.pdf','code-civil-1951-irq'),
  ('brocardi.it/codice-civile','code-civil-ita'),
  ('translated/code-de-procedure-penale-italien-it','code-de-procedure-penale-ita'),
  ('brocardi.it/codice-penale','code-penal-ita'),
  ('brocardi.it/codice-di-procedura-civile','code-procedure-civile-ita'),
  ('cdpc.univ-tln.fr/constit_italie_2022','constitution-1947-ita'),
  ('jafbase/docUE/Italie/LoiDIP.pdf','loi-dip-218-1995-ita'),
  ('jafbase/docAsie/Jordanie/code civil jordanien 1976.pdf','code-civil-1976-jor'),
  ('jafbase/docAfrique/Comores/code de la famille comorien.pdf','code-de-la-famille-com'),
  ('african/code-de-la-nationalite-comorienne-km','code-de-la-nationalite-com'),
  ('african/code-de-l-etat-civil-comorien-km','code-de-l-etat-civil-com'),
  ('clr.africanchildforum.org/COM-loi-84-10-etat-civil','loi-84-10-etat-civil-com'),
  ('foreign/code-obligations-contrats-libanais-lb','code-obligations-contrats-lbn'),
  ('jafbase/docAsie/Liban/non musulmans textes.pdf','loi-sur-le-statut-personnel-successions-des-non-musulmans-lbn'),
  ('jafbase/docAsie/Liban/lois communes.PDF','lois-etat-civil-communes-lbn'),
  ('legilux/eli/etat/leg/code/civil','code-civil-lux'),
  ('jafbase/docMaghreb/MarocCodeFam-oct2005.pdf','code-de-la-famille-moudawana-2004-mar'),
  ('african/code-de-la-nationalite-marocaine-ma','code-de-la-nationalite-mar'),
  ('african/code-de-l-etat-civil-marocain-ma','code-de-l-etat-civil-mar'),
  ('african/code-de-procedure-civile-marocain-ma','code-de-procedure-civile-mar'),
  ('african/code-de-procedure-penale-marocain-ma','code-de-procedure-penale-mar'),
  ('casainvest.ma/MA-code-travail-65-99-BO5210','code-du-travail-loi-65-99-mar'),
  ('rabat.eregulations.org/MAR-DOC-1913','code-obligations-contrats-doc-mar'),
  ('onousc.ma/MA-code-penal-1962-consolide-2018','code-penal-1962-mar'),
  ('african/loi-kafala-marocaine-15-01-ma','loi-kafala-15-01-mar'),
  ('monaco/loi-1448-2017-dip','loi-1448-2017-dip-mco'),
  ('african/code-de-l-etat-civil-malgache-mg','code-de-l-etat-civil-mdg'),
  ('botschaft-madagaskar.de/MDG-code-nationalite-60-064','code-nationalite-mdg'),
  ('jafbase/docAfrique/Madagascar/Adoption - loi d''application Madagascar.pdf','decret-2006-596-application-loi-adoption-mdg'),
  ('cnlegis.gov.mg/L2018-027','loi-2018-027-etat-civil-mdg'),
  ('african/loi-filiation-malgache-mg','loi-filiation-mdg'),
  ('jafbase/docAfrique/Madagascar/Loi sur l''adoption malgache.pdf','loi-n-2005-014-du-7-septembre-2005-relative-a-l-adoption-mdg'),
  ('jafbase/docAfrique/Madagascar/LoiMariage.pdf','loi-n-2007-022-du-20-aout-2007-relative-au-mariage-et-aux-regimes-matrimoniaux-mdg'),
  ('jafbase/docAfrique/Madagascar/Loi droits et protection des enfants Madagascar.pdf','loi-n-2007-023-du-20-aout-2007-sur-les-droits-et-la-protection-des-enfants-mdg'),
  ('jafbase/docAfrique/Madagascar/LoiDIP.pdf','ordonnance-n-62-041-du-19-septembre-1962-relative-aux-dispositions-generales-de-droit-interne-et-de-droit-international-prive-mdg'),
  ('african/code-de-la-nationalite-malienne-ml','code-de-la-nationalite-mli'),
  ('sgg-mali.ml/CPCCS-1999-maj2009','code-de-procedure-civile-commerciale-et-sociale-mli'),
  ('jafbase/docAfrique/Mali/Code_des_Personnes_et_de_la_Famille nouveau.pdf','code-des-personnes-et-de-la-famille-loi-n2011-087-mli'),
  ('african/code-penal-malien-ml','code-penal-mli'),
  ('african/code-procedure-penale-malien-ml','code-procedure-penale-mli'),
  ('african/code-parente-mali-ordonnance-73-036-ml','code-parente-mli'),
  ('african/loi-etat-civil-malienne-06-024-ml','loi-etat-civil-06-024-mli'),
  ('african/loi-etat-civil-malienne-87-27-ml','loi-etat-civil-87-27-mli'),
  ('african/code-de-la-nationalite-mauritanienne-mr','code-de-la-nationalite-mrt'),
  ('african/code-de-l-etat-civil-mauritanien-2011-mr','code-de-l-etat-civil-2011-mrt'),
  ('african/code-de-l-etat-civil-mauritanien-mr','code-de-l-etat-civil-mrt'),
  ('jafbase/docAfrique/Mauritanie/CodeStatutPerso.pdf','code-du-statut-personnel-loi-n-2001-052-mrt'),
  ('jafbase/docAfrique/Maurice/MauriceCodeCivil.pdf','code-civil-mus'),
  ('NC/CODE-IMPOTS','code-impots-ncl'),
  ('african/code-de-la-nationalite-nigerienne-ne','code-de-la-nationalite-ner'),
  ('african/code-de-procedure-civile-niger-ne','code-de-procedure-civile-ner'),
  ('african/code-procedure-penale-nigerien-ne','code-procedure-penale-ner'),
  ('african/loi-2019-29-etat-civil-niger-ne','loi-2019-29-etat-civil-ner'),
  ('jafbase/docAfrique/Nigeria/Loi_LAGOS_NGA_Fr adoption.pdf','loi-sur-adoption-etat-de-lagos-nga'),
  ('translated/code-civil-neerlandais-nl','code-civil-nld'),
  ('translated/code-de-procedure-civile-neerlandais-nl','code-de-procedure-civile-nld'),
  ('translated/code-penal-neerlandais','code-penal-nld'),
  ('translated/constitution-neerlandaise','constitution-nld'),
  ('jafbase/DocAmeriques/Perou/codecivil.PDF','code-civil-per'),
  ('jafbase/DocAmeriques/Perou/LoiDivorceNonContentieux.pdf','loi-divorce-29227-per'),
  ('lexpol.cloud.pf/code-amenagement-pf','code-amenagement-pyf'),
  ('lexpol.cloud.pf/code-impots-pf','code-des-impots-pyf'),
  ('translated/code-civil-polonais-pl','code-civil-pol'),
  ('translated/code-de-la-famille-polonais','code-de-la-famille-pol'),
  ('translated/code-du-travail-polonais-pl','code-du-travail-pol'),
  ('translated/code-penal-polonais','code-penal-pol'),
  ('mjp.univ-perp.fr/pl1997','constitution-1997-pol'),
  ('pgdlisboa.pt/codigo-civil','code-civil-prt'),
  ('translated/code-de-procedure-civile-portugais-pt','code-de-procedure-civile-prt'),
  ('translated/code-de-procedure-penale-portugais','code-de-procedure-penale-prt'),
  ('parlamento.pt/CRP-7e-revision-2005','constitution-1976-prt'),
  ('translated/code-civil-roumain','code-civil-rou'),
  ('translated/code-de-procedure-civile-roumain','code-de-procedure-civile-rou'),
  ('translated/code-penal-roumain','code-penal-rou'),
  ('mjp.univ-perp.fr/ro2003','constitution-2003-rou'),
  ('jafbase/docEstEurope/Serbie/Loi familiale (porodicnizakonfrancuski) non officielle.pdf','loi-familiale-srb'),
  ('zakonrf.info/gk','code-civil-rus'),
  ('translated/code-de-procedure-penale-russe-ru','code-de-procedure-penale-rus'),
  ('mjp.univ-perp.fr/ru1993','constitution-1993-rus'),
  ('foreign/code-penal-rwandais-rw','code-penal-rwa'),
  ('jafbase/docAfrique/Senegal/CODE_FAMILLE sénégal 2018.pdf','code-de-la-famille-sen'),
  ('african/code-de-la-nationalite-senegalaise-sn','code-de-la-nationalite-sen'),
  ('african/code-de-procedure-civile-senegalais-sn','code-de-procedure-civile-sen'),
  ('justice.sec.gouv.sn/code-procedure-penal','code-de-procedure-penale-sen'),
  ('natlex.ilo.org/SEN-code-travail-97-17','code-du-travail-sen'),
  ('african/code-etat-civil-senegalais-sn','code-etat-civil-sen'),
  ('african/code-penal-senegalais-sn','code-penal-sen'),
  ('jafbase/docAfrique/Sao Tome/Loi famille.pdf','code-famille-2-77-stp'),
  ('jafbase/docAsie/Syrie/CodeCivDIP.pdf','code-civil-syr'),
  ('jafbase/docAsie/Syrie/Code civil syrien 1949.pdf','code-civil-1949-syr'),
  ('jafbase/docAsie/Syrie/code du statut personnel des syriens orthodoxes.pdf','code-statut-personnel-syriens-orthodoxes-syr'),
  ('jafbase/docAsie/Syrie/loi sur les sectes religieuses 1936.pdf','loi-communautes-religieuses-60-1936-syr'),
  ('jafbase/docAfrique/Tchad/code civil Tchad 1959.pdf','code-civil-tcd'),
  ('african/code-de-la-nationalite-tchadienne-td','code-de-la-nationalite-tcd'),
  ('jafbase/docAfrique/Tchad/Loi 29 mai 2013 portant code de l''organisation judiciaire.pdf','code-de-lorganisation-judiciaire-tcd'),
  ('african/code-de-procedure-civile-tchadien-td','code-de-procedure-civile-tcd'),
  ('african/code-de-procedure-penale-tchadien-td','code-de-procedure-penale-tcd'),
  ('african/ordonnance-002-pr-2020-etat-civil-tchad-td','ordonnance-002-pr-2020-etat-civil-tcd'),
  ('jafbase/docAfrique/Togo/CodeCiv.pdf','code-civil-tgo'),
  ('african/code-de-la-nationalite-togolaise-tg','code-de-la-nationalite-tgo'),
  ('african/code-des-personnes-et-de-la-famille-togo-tg','code-des-personnes-et-de-la-famille-tgo'),
  ('african/code-procedure-civile-togolais-tg','code-procedure-civile-tgo'),
  ('african/code-procedure-penale-togolais-tg','code-procedure-penale-tgo'),
  ('african/loi-2009-010-etat-civil-togo-tg','loi-2009-010-etat-civil-tgo'),
  ('african/code-de-droit-international-prive-tunisien-tn','code-de-droit-international-prive-tun'),
  ('african/code-de-la-nationalite-tunisienne-tn','code-de-la-nationalite-tun'),
  ('african/code-de-l-etat-civil-tunisien-tn','code-de-l-etat-civil-tun'),
  ('jafbase/docMaghreb/TunisieStatutpersonnel.PDF','code-du-statut-personnel-tun'),
  ('cmf.tn/TN-code-incitations-investissements-93-120','code-incitations-investissements-tun'),
  ('cabinetnfm.com/TUN-COC-1906','code-obligations-contrats-coc-tun'),
  ('african/code-penal-tunisien-tn','code-penal-tun'),
  ('mjp.univ-perp.fr/tn2014','constitution-2014-tun'),
  ('translated/code-civil-turc-tr','code-civil-tur'),
  ('mjp.univ-perp.fr/tr1982','constitution-1982-tur'),
  ('translated/code-de-procedure-penale-ukrainien-ua','code-de-procedure-penale-ukr'),
  ('mjp.univ-perp.fr/ua1996','constitution-1996-ukr'),
  ('translated/constitution-venezuelienne','constitution-ven'),
  ('jafbase/DocAmeriques/Venezuela/LoiDIP.PDF','loi-dip-1998-ven'),
  ('jafbase/docAsie/Vietnam/Code civil vietnam 2015 FR.pdf','code-civil-2015-vnm'),
  ('jafbase/docAsie/Vietnam/DecretEnregistrementMariages.pdf','decret-77-2001-enregistrement-mariage-vnm'),
  ('jafbase/docAsie/Vietnam/LoiFamille.pdf','loi-mariage-famille-2000-vnm')
)
UPDATE legal_text t
SET slug = r.new_slug
FROM renames r
WHERE t.text_uid = r.text_uid;

UPDATE legal_text
SET slug = NULL
WHERE slug ~ '-(cir_[0-9]+|jorftext[0-9]+|legitext[0-9]+)$';

-- 0142_jurisdiction_type_rename.sql
SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

ALTER TABLE decisions RENAME COLUMN juridiction_type TO jurisdiction_type;
ALTER TABLE decision_chunks RENAME COLUMN juridiction_type TO jurisdiction_type;
ALTER TABLE jurisdiction RENAME COLUMN juridiction_type TO jurisdiction_type;

DROP INDEX decisions_bm25;

CREATE INDEX decisions_bm25 ON decisions USING bm25 (id, full_text, search_title, ((jurisdiction_type)::pdb.literal), ((legal_instruments)::pdb.literal), ((legal_article_composite)::pdb.literal), ((publication_codes)::pdb.literal), date_lecture) WITH (key_field=id, text_fields='{
    "full_text":    {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"},
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}
  }');

ANALYZE decisions;

-- 0143_search_history_jurisdiction_type_key.sql
UPDATE user_search_history
SET filters = (filters - 'juridictionType' - 'juridiction_type')
              || jsonb_build_object(
                     'jurisdictionType',
                     COALESCE(filters -> 'juridictionType', filters -> 'juridiction_type')
                 )
WHERE filters ?| ARRAY['juridictionType', 'juridiction_type'];

-- 0144_article_commentary.sql
CREATE TABLE IF NOT EXISTS article_commentaire (
    id               BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    text_uid         TEXT NOT NULL,
    num_key          TEXT,
    source           TEXT NOT NULL,
    source_uid       TEXT NOT NULL UNIQUE,
    content_checksum TEXT NOT NULL,
    payload_format   TEXT NOT NULL DEFAULT 'json',
    source_fields    JSONB NOT NULL,
    ingested_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at       TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS article_commentaire_anchor_idx
    ON article_commentaire (text_uid, num_key)
    WHERE deleted_at IS NULL;

-- 0145_facet_value_axis_rename.sql
SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

DROP INDEX decisions_bm25;

ALTER TABLE decisions
  DROP CONSTRAINT decisions_chambre_uid_fkey,
  DROP CONSTRAINT decisions_voie_uid_fkey,
  DROP CONSTRAINT decisions_legal_domain_uid_fkey,
  DROP CONSTRAINT decisions_voie_uid_check,
  DROP CONSTRAINT decisions_legal_domain_uid_check;

UPDATE facet_value SET
  uid = CASE facet
    WHEN 'juridiction' THEN 'jurisdiction_type' || substr(uid, 12)
    WHEN 'domaine' THEN 'legal_domain' || substr(uid, 8)
    WHEN 'chambre' THEN 'chamber' || substr(uid, 8)
    WHEN 'voie' THEN 'procedure' || substr(uid, 5)
    WHEN 'portee' THEN 'significance' || substr(uid, 7)
  END,
  parent_uid = CASE
    WHEN parent_uid LIKE 'domaine:%' THEN 'legal_domain' || substr(parent_uid, 8)
    WHEN parent_uid LIKE 'juridiction:%' THEN 'jurisdiction_type' || substr(parent_uid, 12)
    ELSE parent_uid
  END,
  facet = CASE facet
    WHEN 'juridiction' THEN 'jurisdiction_type'
    WHEN 'domaine' THEN 'legal_domain'
    WHEN 'chambre' THEN 'chamber'
    WHEN 'voie' THEN 'procedure'
    WHEN 'portee' THEN 'significance'
  END
WHERE facet IN ('juridiction', 'domaine', 'chambre', 'voie', 'portee');

UPDATE decisions SET
  chambre_uid = CASE WHEN chambre_uid IS NULL THEN NULL
    ELSE 'chamber' || substr(chambre_uid, 8) END,
  voie_uid = CASE WHEN voie_uid IS NULL THEN NULL
    ELSE 'procedure' || substr(voie_uid, 5) END,
  legal_domain_uid = CASE WHEN legal_domain_uid IS NULL THEN NULL
    ELSE 'legal_domain' || substr(legal_domain_uid, 8) END
WHERE chambre_uid IS NOT NULL
   OR voie_uid IS NOT NULL
   OR legal_domain_uid IS NOT NULL;

ALTER TABLE decisions RENAME COLUMN chambre_uid TO chamber_uid;
ALTER TABLE decisions RENAME COLUMN voie_uid TO procedure_uid;

ALTER TABLE decisions
  ADD CONSTRAINT decisions_chamber_uid_fkey
    FOREIGN KEY (chamber_uid) REFERENCES facet_value(uid),
  ADD CONSTRAINT decisions_procedure_uid_fkey
    FOREIGN KEY (procedure_uid) REFERENCES facet_value(uid),
  ADD CONSTRAINT decisions_legal_domain_uid_fkey
    FOREIGN KEY (legal_domain_uid) REFERENCES facet_value(uid),
  ADD CONSTRAINT decisions_procedure_uid_check
    CHECK (procedure_uid LIKE 'procedure:%'),
  ADD CONSTRAINT decisions_legal_domain_uid_check
    CHECK (legal_domain_uid LIKE 'legal_domain:%');

UPDATE user_search_history
SET filters = (filters - 'chambre') || jsonb_build_object('chamber', filters -> 'chambre')
WHERE filters ? 'chambre';
UPDATE user_search_history
SET filters = (filters - 'voie') || jsonb_build_object('procedure', filters -> 'voie')
WHERE filters ? 'voie';
UPDATE user_search_history
SET filters = (filters - 'portee') || jsonb_build_object('significance', filters -> 'portee')
WHERE filters ? 'portee';
UPDATE user_search_history
SET filters = (filters - 'ai_mode') || jsonb_build_object('aiMode', filters -> 'ai_mode')
WHERE filters ? 'ai_mode';
UPDATE user_search_history
SET filters = (filters - 'date_from') || jsonb_build_object('dateFrom', filters -> 'date_from')
WHERE filters ? 'date_from';
UPDATE user_search_history
SET filters = (filters - 'date_to') || jsonb_build_object('dateTo', filters -> 'date_to')
WHERE filters ? 'date_to';
UPDATE user_search_history
SET filters = (filters - 'legal_article') || jsonb_build_object('legalArticle', filters -> 'legal_article')
WHERE filters ? 'legal_article';
UPDATE user_search_history
SET filters = (filters - 'legal_instrument') || jsonb_build_object('legalInstrument', filters -> 'legal_instrument')
WHERE filters ? 'legal_instrument';
UPDATE user_search_history
SET filters = filters - ARRAY[
  'mainOutcome', 'main_outcome', 'specialProcedure', 'special_procedure',
  'jurisdictionName', 'jurisdiction_name', 'instanceLevel', 'instance_level',
  'jurisdictionLevel', 'jurisdiction_level', 'publicationCodes',
  'is_tables_lebon', 'is_recueil'
]
WHERE filters ?| ARRAY[
  'mainOutcome', 'main_outcome', 'specialProcedure', 'special_procedure',
  'jurisdictionName', 'jurisdiction_name', 'instanceLevel', 'instance_level',
  'jurisdictionLevel', 'jurisdiction_level', 'publicationCodes',
  'is_tables_lebon', 'is_recueil'
];

CREATE INDEX decisions_bm25 ON decisions USING bm25 (id, full_text, search_title, ((jurisdiction_type)::pdb.literal), ((legal_instruments)::pdb.literal), ((legal_article_composite)::pdb.literal), ((publication_codes)::pdb.literal), date_lecture) WITH (key_field=id, text_fields='{
    "full_text":    {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"},
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}
  }');

ANALYZE decisions;
ANALYZE facet_value;

-- 0146_suggest_index.sql
CREATE TABLE suggest_index (
    key      text        PRIMARY KEY,
    built_at timestamptz NOT NULL DEFAULT now(),
    fst      bytea       NOT NULL
);

-- 0147_text_legal_citation.sql
CREATE TABLE text_legal_citation (
    owner_text_uid   text NOT NULL REFERENCES legal_text(text_uid) ON DELETE CASCADE,
    owner_num_key    text,
    owner_date_debut date,
    char_start       int4 NOT NULL,
    char_end         int4 NOT NULL,
    ref_text_uid     text NOT NULL REFERENCES legal_text(text_uid) ON DELETE CASCADE,
    ref_num_key      text,
    extract_version  int2 NOT NULL,
    CHECK (char_end > char_start),
    CHECK ((owner_num_key IS NULL) = (owner_date_debut IS NULL))
);

CREATE UNIQUE INDEX text_legal_citation_owner_span_key
    ON text_legal_citation (owner_text_uid, COALESCE(owner_num_key, ''),
                            COALESCE(owner_date_debut, '0001-01-01'::date), char_start);

-- 0148_purge_legal_text_version_shells.sql
CREATE INDEX IF NOT EXISTS idx_tlc_ref ON text_legal_citation (ref_text_uid);

DELETE FROM legal_link ll
USING legal_text lt
WHERE ll.owner_text_uid = lt.text_uid
  AND lt.text_uid LIKE 'LEGITEXT%'
  AND (lt.body IS NULL OR length(lt.body) < 300)
  AND NOT EXISTS (SELECT 1 FROM legal_article la WHERE la.text_uid = lt.text_uid);

UPDATE legal_citation lc SET ref_text_uid = NULL
FROM legal_text lt
WHERE lc.ref_text_uid = lt.text_uid
  AND lt.text_uid LIKE 'LEGITEXT%'
  AND (lt.body IS NULL OR length(lt.body) < 300)
  AND NOT EXISTS (SELECT 1 FROM legal_article la WHERE la.text_uid = lt.text_uid);

DELETE FROM legal_text lt
WHERE lt.text_uid LIKE 'LEGITEXT%'
  AND (lt.body IS NULL OR length(lt.body) < 300)
  AND NOT EXISTS (SELECT 1 FROM legal_article la WHERE la.text_uid = lt.text_uid);

-- 0149_legal_citation_family_expansion.sql
ALTER TABLE legal_citation ADD COLUMN suivants boolean NOT NULL DEFAULT false;

CREATE INDEX idx_legal_toc_edge_art_num
    ON legal_toc_edge (text_uid, child_num_key) WHERE child_kind = 'article';

CREATE FUNCTION _suivants_family_keys(p_uid text, p_num_key text)
RETURNS text[]
LANGUAGE sql STABLE
AS $$
    WITH anchor AS (
        SELECT owner_uid, min(seq) AS seq
        FROM legal_toc_edge
        WHERE text_uid = p_uid AND child_kind = 'article'
          AND child_num_key = p_num_key AND etat = 'VIGUEUR'
        GROUP BY owner_uid
    ),
    members AS (
        SELECT a.owner_uid, e.child_num_key, min(e.seq) AS seq_min
        FROM anchor a
        JOIN legal_toc_edge e
          ON e.owner_uid = a.owner_uid
         AND e.child_kind = 'article' AND e.etat = 'VIGUEUR'
         AND e.seq >= a.seq
        GROUP BY a.owner_uid, e.child_num_key
    ),
    fams AS (
        SELECT owner_uid, array_agg(child_num_key ORDER BY seq_min) AS fam
        FROM members
        GROUP BY owner_uid
    )
    SELECT CASE WHEN count(DISTINCT fam) = 1
                 AND cardinality(min(fam)) BETWEEN 2 AND 20
                THEN min(fam)
           END
    FROM fams;
$$;

CREATE FUNCTION _suivants_family(p_uid text, p_cited text)
RETURNS text[]
LANGUAGE sql STABLE
AS $$
    WITH pub AS (
        SELECT CASE
            WHEN p_cited ~ '^[LRDA]\. ?\d[0-9-]*$'
                THEN lower(regexp_replace(p_cited, '\. ?', ''))
            WHEN p_cited ~ '^\d[0-9-]*$' THEN p_cited
        END AS anchor_key
    ),
    fam AS (
        SELECT unnest(_suivants_family_keys(p_uid, pub.anchor_key)) AS k
        FROM pub WHERE pub.anchor_key IS NOT NULL
    ),
    citable AS (
        SELECT CASE
            WHEN k ~ '^[lrda]\d[0-9-]*$'
                THEN upper(left(k, 1)) || '. ' || substr(k, 2)
            WHEN k ~ '^\d[0-9-]*$' THEN k
        END AS c
        FROM fam
    )
    SELECT CASE WHEN count(c) >= 2 THEN array_agg(c) END
    FROM citable WHERE c IS NOT NULL;
$$;

CREATE OR REPLACE FUNCTION _sync_decisions_legal_instruments_for(p_ids bigint[])
RETURNS bigint
LANGUAGE sql
AS $$
    WITH pairs AS (
        SELECT lc.decision_id, lc.ref_text_uid,
               lc.ref_text_uid || '|' || k AS composite
        FROM legal_citation lc
        LEFT JOIN LATERAL unnest(
            CASE WHEN lc.suivants AND lc.ref_num_key IS NOT NULL
                 THEN coalesce(_suivants_family(lc.ref_text_uid, lc.ref_num_key),
                               ARRAY[lc.ref_num_key])
                 ELSE ARRAY[lc.ref_num_key] END
        ) AS k ON true
        WHERE lc.decision_id = ANY(p_ids)
    ),
    agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT p.ref_text_uid COLLATE "C"
                      ORDER BY p.ref_text_uid COLLATE "C")
                FILTER (WHERE p.ref_text_uid IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT p.composite COLLATE "C"
                      ORDER BY p.composite COLLATE "C")
                FILTER (WHERE p.composite IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN pairs p ON p.decision_id = t.id
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
    WITH pairs AS (
        SELECT lc.decision_id, lc.ref_text_uid,
               lc.ref_text_uid || '|' || k AS composite
        FROM legal_citation lc
        LEFT JOIN LATERAL unnest(
            CASE WHEN lc.suivants AND lc.ref_num_key IS NOT NULL
                 THEN coalesce(_suivants_family(lc.ref_text_uid, lc.ref_num_key),
                               ARRAY[lc.ref_num_key])
                 ELSE ARRAY[lc.ref_num_key] END
        ) AS k ON true
        WHERE lc.decision_id = ANY(p_ids)
    ),
    agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT p.ref_text_uid COLLATE "C"
                      ORDER BY p.ref_text_uid COLLATE "C")
                FILTER (WHERE p.ref_text_uid IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT p.composite COLLATE "C"
                      ORDER BY p.composite COLLATE "C")
                FILTER (WHERE p.composite IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN pairs p ON p.decision_id = t.id
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

-- 0150_directory_registry_totals.sql
CREATE TABLE annuaire_registre (

    category text PRIMARY KEY,
    total    bigint NOT NULL
);

INSERT INTO annuaire_registre (category, total)
SELECT CASE
         WHEN uid LIKE 'siren:%' AND nature = 'morale_privee'
           THEN 'entreprises'
         WHEN uid LIKE 'siren:%' AND nature = 'morale_publique'
           THEN 'personnes_publiques'
         WHEN uid LIKE 'rna:%' THEN 'associations'
         WHEN uid LIKE 'cnb:%' OR uid LIKE 'oacc:%' THEN 'avocats'
       END AS category,
       count(*)
FROM entity
WHERE uid LIKE 'siren:%' OR uid LIKE 'rna:%'
   OR uid LIKE 'cnb:%' OR uid LIKE 'oacc:%'
GROUP BY 1;

-- 0151_legal_citation_family_public_keys.sql
CREATE OR REPLACE FUNCTION _sync_decisions_legal_instruments_for(p_ids bigint[])
RETURNS bigint
LANGUAGE sql
AS $$
    WITH pairs AS (
        SELECT lc.decision_id, lc.ref_text_uid,
               lc.ref_text_uid || '|' || k AS composite
        FROM legal_citation lc
        LEFT JOIN LATERAL unnest(
            CASE WHEN lc.suivants AND lc.ref_num_key IS NOT NULL
                 THEN coalesce(_suivants_family_keys(lc.ref_text_uid, lc.ref_num_key),
                               ARRAY[lc.ref_num_key])
                 ELSE ARRAY[lc.ref_num_key] END
        ) AS k ON true
        WHERE lc.decision_id = ANY(p_ids)
    ),
    agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT p.ref_text_uid COLLATE "C"
                      ORDER BY p.ref_text_uid COLLATE "C")
                FILTER (WHERE p.ref_text_uid IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT p.composite COLLATE "C"
                      ORDER BY p.composite COLLATE "C")
                FILTER (WHERE p.composite IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN pairs p ON p.decision_id = t.id
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
    WITH pairs AS (
        SELECT lc.decision_id, lc.ref_text_uid,
               lc.ref_text_uid || '|' || k AS composite
        FROM legal_citation lc
        LEFT JOIN LATERAL unnest(
            CASE WHEN lc.suivants AND lc.ref_num_key IS NOT NULL
                 THEN coalesce(_suivants_family_keys(lc.ref_text_uid, lc.ref_num_key),
                               ARRAY[lc.ref_num_key])
                 ELSE ARRAY[lc.ref_num_key] END
        ) AS k ON true
        WHERE lc.decision_id = ANY(p_ids)
    ),
    agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT p.ref_text_uid COLLATE "C"
                      ORDER BY p.ref_text_uid COLLATE "C")
                FILTER (WHERE p.ref_text_uid IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT p.composite COLLATE "C"
                      ORDER BY p.composite COLLATE "C")
                FILTER (WHERE p.composite IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN pairs p ON p.decision_id = t.id
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

DROP FUNCTION _suivants_family(text, text);

-- 0152_mcp_refresh_tokens.sql
CREATE TABLE mcp_refresh_tokens (
    refresh_token TEXT PRIMARY KEY,
    user_id       TEXT NOT NULL REFERENCES users (sub) ON DELETE CASCADE,
    client_id     TEXT NOT NULL REFERENCES mcp_clients (client_id) ON DELETE CASCADE,
    expires_at    TIMESTAMPTZ NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX ON mcp_refresh_tokens (expires_at);

-- 0153_entity_directory_complete.sql
ALTER TABLE entity
    ADD COLUMN category text,
    ADD COLUMN ape text,
    ADD COLUMN barreau text,
    ADD COLUMN decision_count bigint NOT NULL DEFAULT 0;

UPDATE entity SET
    category = CASE
        WHEN uid LIKE 'siren:%' AND nature = 'morale_privee' THEN 'entreprises'
        WHEN uid LIKE 'siren:%' AND nature = 'morale_publique' THEN 'personnes_publiques'
        WHEN uid LIKE 'rna:%' THEN 'associations'
        WHEN uid LIKE 'oacc:firm-%' THEN 'cabinets'
        WHEN uid LIKE 'cnb:%' OR uid LIKE 'oacc:%' THEN 'avocats'
    END,
    barreau = CASE WHEN uid LIKE 'cnb:%' THEN split_part(uid, ':', 2) END;

ALTER TABLE entity ALTER COLUMN category SET NOT NULL;

UPDATE entity e
SET decision_count = c.n
FROM (
    SELECT p.entity_uid, count(DISTINCT p.decision_id) AS n
    FROM decision_party p
    JOIN decisions d ON d.id = p.decision_id
    WHERE p.entity_uid IS NOT NULL AND d.deleted_at IS NULL
    GROUP BY p.entity_uid
) c
WHERE e.uid = c.entity_uid;

CREATE INDEX entity_annuaire_idx
    ON entity (category, decision_count DESC, denomination_folded);

CREATE INDEX entity_barreau_idx
    ON entity (barreau, decision_count DESC, denomination_folded)
    WHERE barreau IS NOT NULL;

CREATE INDEX entity_denomination_prefix_idx
    ON entity (denomination_folded text_pattern_ops);
DROP INDEX entity_denomination_folded_idx;

ALTER TABLE entity SET (fillfactor = 90);

ALTER TABLE annuaire_registre ADD COLUMN contentieux bigint NOT NULL DEFAULT 0;
TRUNCATE annuaire_registre;
INSERT INTO annuaire_registre (category, total, contentieux)
SELECT category, count(*), count(*) FILTER (WHERE decision_count > 0)
FROM entity
GROUP BY category;

DROP TABLE entity_contentieux;

-- 0154_entity_search_litigation_index.sql
CREATE INDEX entity_denomination_contentieux_idx
    ON entity (denomination_folded text_pattern_ops)
    WHERE decision_count > 0;

-- 0155_entity_single_table_fold_index.sql
CREATE FUNCTION lj_fold(text) RETURNS text
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT
AS $fn$
SELECT btrim(regexp_replace(
    lower(translate($1,
        'ÀÂÄàâäÉÈÊËéèêëÎÏîïÔÖôöÙÛÜùûüÇçŒœ'
        || U&'\2019\0009\000A\000B\000C\000D\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000',
        'aaaaaaeeeeeeeeiiiioooouuuuuuccoo'
        || '''' || '                        ')),
    '\s+', ' ', 'g'))
$fn$;

CREATE FUNCTION lj_fold_all(jsonb) RETURNS text[]
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT
AS $fn$
SELECT coalesce(array_agg(public.lj_fold(e->>'d')), '{}')
FROM jsonb_array_elements($1) AS e
$fn$;

DO $$
DECLARE bad bigint;
BEGIN
    SELECT count(*) INTO bad FROM entity
    WHERE lj_fold(denomination) IS DISTINCT FROM denomination_folded;
    IF bad > 0 THEN
        RAISE EXCEPTION 'lj_fold diverge de canon() sur % lignes entity', bad;
    END IF;
    SELECT count(*) INTO bad FROM entity_denomination
    WHERE lj_fold(denomination) IS DISTINCT FROM folded;
    IF bad > 0 THEN
        RAISE EXCEPTION 'lj_fold diverge de canon() sur % lignes entity_denomination', bad;
    END IF;
END $$;

ALTER TABLE entity ADD COLUMN denominations jsonb;

UPDATE entity e SET denominations = d.arr
FROM (
    SELECT entity_uid,
           jsonb_agg(DISTINCT jsonb_strip_nulls(jsonb_build_object(
               'd', denomination,
               'du', date_debut::text,
               'au', date_fin::text))) AS arr
    FROM entity_denomination
    GROUP BY entity_uid
) d
WHERE d.entity_uid = e.uid;

UPDATE entity SET denominations = jsonb_build_array(jsonb_build_object('d', denomination))
WHERE denominations IS NULL;

ALTER TABLE entity ALTER COLUMN denominations SET NOT NULL;

DROP INDEX entity_annuaire_idx;
DROP INDEX entity_denomination_prefix_idx;
DROP INDEX entity_denomination_contentieux_idx;
ALTER TABLE entity DROP COLUMN denomination_folded;

CREATE INDEX entity_annuaire_idx
    ON entity (category, decision_count DESC, lj_fold(denomination));
CREATE INDEX entity_prefix_idx
    ON entity (lj_fold(denomination) text_pattern_ops);
CREATE INDEX entity_prefix_contentieux_idx
    ON entity (lj_fold(denomination) text_pattern_ops)
    WHERE decision_count > 0;
CREATE INDEX entity_folded_all_idx
    ON entity USING gin (lj_fold_all(denominations));

DROP TABLE entity_denomination;

-- 0156_mistral_key_status.sql
CREATE TABLE mistral_key_status (
    fingerprint text PRIMARY KEY,
    disabled_until timestamptz NOT NULL,
    last_status smallint NOT NULL,
    marked_by text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);

-- 0157_legal_text_role_alias_of.sql
ALTER TABLE legal_text
    ADD COLUMN role text NOT NULL DEFAULT 'instrument'
        CONSTRAINT legal_text_role_check CHECK (role IN
            ('instrument', 'modificatif', 'vehicule', 'habilitation', 'individuel')),
    ADD COLUMN alias_of text REFERENCES legal_text (text_uid);

CREATE INDEX legal_text_role_idx ON legal_text (role)
    WHERE role <> 'instrument';
CREATE INDEX legal_text_alias_of_idx ON legal_text (alias_of)
    WHERE alias_of IS NOT NULL;

-- 0158_legal_citation_jsonb_per_decision.sql
CREATE FUNCTION lj_cit_terms(jsonb) RETURNS text[]
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT
AS $fn$
SELECT coalesce(array_agg(DISTINCT term), '{}')
FROM (
    SELECT el->>2 AS term FROM jsonb_array_elements($1) AS el
    WHERE el->>2 IS NOT NULL
    UNION
    SELECT (el->>2) || '|' || (el->>3) FROM jsonb_array_elements($1) AS el
    WHERE el->>3 IS NOT NULL
) t
$fn$;

CREATE TABLE legal_citation_blob (
    decision_id     bigint   PRIMARY KEY REFERENCES decisions(id) ON DELETE CASCADE,
    extract_version smallint NOT NULL,
    spans           jsonb    NOT NULL
);

INSERT INTO legal_citation_blob (decision_id, extract_version, spans)
SELECT decision_id,
       max(extract_version),
       jsonb_agg(jsonb_build_array(char_start, char_end, ref_text_uid,
                                   ref_num_key, suivants)
                 ORDER BY char_start)
FROM legal_citation
GROUP BY decision_id;

CREATE INDEX legal_citation_terms_idx
    ON legal_citation_blob USING gin (public.lj_cit_terms(spans));

DROP TABLE legal_citation;
ALTER TABLE legal_citation_blob RENAME TO legal_citation;
ALTER INDEX legal_citation_blob_pkey RENAME TO legal_citation_pkey;
ALTER TABLE legal_citation
    RENAME CONSTRAINT legal_citation_blob_decision_id_fkey
    TO legal_citation_decision_id_fkey;

CREATE OR REPLACE FUNCTION _sync_decisions_legal_instruments_for(p_ids bigint[])
RETURNS bigint
LANGUAGE sql
AS $$
    WITH pairs AS (
        SELECT lc.decision_id, el->>2 AS ref_text_uid,
               (el->>2) || '|' || k AS composite
        FROM legal_citation lc
        CROSS JOIN LATERAL jsonb_array_elements(lc.spans) AS el
        LEFT JOIN LATERAL unnest(
            CASE WHEN (el->>4)::bool AND el->>3 IS NOT NULL
                 THEN coalesce(_suivants_family_keys(el->>2, el->>3),
                               ARRAY[el->>3])
                 ELSE ARRAY[el->>3] END
        ) AS k ON true
        WHERE lc.decision_id = ANY(p_ids)
    ),
    agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT p.ref_text_uid COLLATE "C"
                      ORDER BY p.ref_text_uid COLLATE "C")
                FILTER (WHERE p.ref_text_uid IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT p.composite COLLATE "C"
                      ORDER BY p.composite COLLATE "C")
                FILTER (WHERE p.composite IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN pairs p ON p.decision_id = t.id
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
    WITH pairs AS (
        SELECT lc.decision_id, el->>2 AS ref_text_uid,
               (el->>2) || '|' || k AS composite
        FROM legal_citation lc
        CROSS JOIN LATERAL jsonb_array_elements(lc.spans) AS el
        LEFT JOIN LATERAL unnest(
            CASE WHEN (el->>4)::bool AND el->>3 IS NOT NULL
                 THEN coalesce(_suivants_family_keys(el->>2, el->>3),
                               ARRAY[el->>3])
                 ELSE ARRAY[el->>3] END
        ) AS k ON true
        WHERE lc.decision_id = ANY(p_ids)
    ),
    agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT p.ref_text_uid COLLATE "C"
                      ORDER BY p.ref_text_uid COLLATE "C")
                FILTER (WHERE p.ref_text_uid IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT p.composite COLLATE "C"
                      ORDER BY p.composite COLLATE "C")
                FILTER (WHERE p.composite IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN pairs p ON p.decision_id = t.id
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

-- 0159_legal_article_usage_terms.sql
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

-- 0160_legal_article_usage_table.sql
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

-- 0161_drop_legal_text_alias_of.sql
ALTER TABLE legal_text DROP COLUMN alias_of;

-- 0162_citing_decision_counts.sql
CREATE TABLE citing_decision_counts (
    cited_term     text   PRIMARY KEY,
    decision_count bigint NOT NULL
);

INSERT INTO citing_decision_counts (cited_term, decision_count)
SELECT term, count(*)
FROM legal_citation, unnest(public.lj_cit_terms(spans)) AS term
GROUP BY term;

-- 0163_user_search_history_engine.sql
ALTER TABLE user_search_history
    ADD COLUMN engine TEXT NOT NULL DEFAULT 'decisions';

