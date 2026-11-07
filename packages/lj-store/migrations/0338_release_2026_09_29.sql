-- 0290_drop_chamber_key_and_chamber_table.sql
DROP INDEX idx_decisions_chamber_key;

ALTER TABLE decisions DROP COLUMN chamber_key;

DROP TABLE chamber;

-- 0291_decisions_bm25_v7_docket_keys.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY decisions_bm25_v7 ON decisions
USING bm25 (
  id,
  full_text,
  search_title,
  ((jurisdiction_type)::pdb.literal),
  ((legal_instruments)::pdb.literal),
  ((legal_article_composite)::pdb.literal),
  ((publication_codes)::pdb.literal),
  ((jurisdiction_code)::pdb.literal),
  ((seat_paths)::pdb.literal),
  ((legal_domain_uid)::pdb.literal),
  ((solution_uid)::pdb.literal),
  ((office_uid)::pdb.literal),
  ((procedure_uid)::pdb.literal),
  (lj_docket_keys(docket_numbers)::pdb.literal('alias=docket_keys')),
  ((ecli)::pdb.literal_normalized('alias=ecli')),
  date_lecture,
  deleted_at
) WITH (key_field=id, text_fields='{"full_text": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}, "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}}');

-- 0292_decisions_bm25_swap_docket_keys.sql
DROP INDEX decisions_bm25;

ALTER INDEX decisions_bm25_v7 RENAME TO decisions_bm25;

-- 0293_decisions_summary_engine.sql
ALTER TABLE decisions ADD COLUMN summary_engine SMALLINT;

COMMENT ON COLUMN decisions.summary_engine IS
  'The (prompt, model) pair that wrote summary. Codes in lj_core::summary::SummaryEngine. NULL predates the column.';

-- 0294_gemini_key_day_quota.sql
CREATE TABLE gemini_key_day (
    fingerprint text    NOT NULL,
    model       text    NOT NULL,
    quota_day   date    NOT NULL,
    spent       integer NOT NULL DEFAULT 0,
    updated_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (fingerprint, model, quota_day)
);

-- 0295_annonce_lien_sonde.sql
CREATE TABLE annonce_lien_sonde (
    moteur   text  NOT NULL,
    kind     text  NOT NULL,
    jour     date  NOT NULL,
    verdicts jsonb NOT NULL,
    exemples jsonb NOT NULL,
    PRIMARY KEY (moteur, kind, jour)
);

-- 0296_legal_article_kind_generated.sql
DROP INDEX legal_article_bm25;

ALTER TABLE legal_article DROP COLUMN kind;

ALTER TABLE legal_article
  ADD COLUMN kind text GENERATED ALWAYS AS (
    CASE WHEN texte ~ '^[Aa] (modifié|abrogé|créé|transféré|dénoncé|annulé|remplacé|périmé) les dispositions suivantes'
         THEN 'edit' ELSE 'article' END
  ) STORED;

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
    (kind::pdb.literal),
    searchable,
    date_debut,
    date_fin
)
WITH (
  key_field = 'id',
  text_fields = '{
    "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "texte":        {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "num":          {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "basic"}
  }'
);

-- 0297_annonce_zones_array.sql
ALTER TABLE annonce ADD COLUMN zones text[] NOT NULL DEFAULT '{}';

CREATE INDEX annonce_zones_gin ON annonce USING gin (zones);

-- 0298_disable_aggregate_custom_scan.sql
DO $$
BEGIN
    EXECUTE format('ALTER DATABASE %I SET paradedb.enable_aggregate_custom_scan = off',
                   current_database());
END $$;

-- 0299_annonce_bm25_index.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY annonce_bm25 ON annonce
USING bm25 (uid, titre, acteur_libelle, fond, famille_uid, cpv_uid, departement)
WITH (
  key_field = 'uid',
  text_fields = '{
    "titre":          {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "acteur_libelle": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "fond":           {"tokenizer": {"type": "raw"}, "fast": true},
    "famille_uid":    {"tokenizer": {"type": "raw"}, "fast": true},
    "cpv_uid":        {"tokenizer": {"type": "raw"}, "fast": true},
    "departement":    {"tokenizer": {"type": "raw"}, "fast": true}
  }'
);

-- 0300_annonce_description_column.sql
ALTER TABLE annonce ADD COLUMN description text;

-- 0301_journal_wording_assertion_tables.sql
CREATE TABLE journal (
    code       text PRIMARY KEY,
    label      text NOT NULL,
    publisher  text,
    languages  text[] NOT NULL DEFAULT '{fr}'
);

CREATE TABLE wording (
    id                  bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    language            text NOT NULL,
    fingerprint         bigint NOT NULL,
    fingerprint_version smallint NOT NULL,
    length              integer NOT NULL,
    carrier_text_uid    text NOT NULL,
    carrier_num_key     text NOT NULL,
    carrier_start       date NOT NULL,
    carrier_original    boolean NOT NULL DEFAULT false,
    UNIQUE (language, fingerprint, fingerprint_version, length)
);

CREATE TABLE assertion (
    id             bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    text_uid       text NOT NULL,
    num_key        text NOT NULL,
    places         text[] NOT NULL,
    starts_on      date NOT NULL,
    ends_on        date,
    start_origin   text NOT NULL CHECK (start_origin IN ('received', 'computed', 'fallback')),
    end_origin     text CHECK (end_origin IN ('received', 'computed')),
    start_source   text,
    end_source     text,
    version_start  date NOT NULL,
    CHECK (ends_on IS NULL OR ends_on > starts_on),
    CHECK ((ends_on IS NULL) = (end_origin IS NULL)),
    CHECK (cardinality(places) > 0)
);
CREATE INDEX assertion_article_idx ON assertion (text_uid, num_key, starts_on);
CREATE INDEX assertion_places_idx ON assertion USING gin (places);

CREATE TABLE expression (
    assertion_id bigint NOT NULL REFERENCES assertion(id) ON DELETE CASCADE,
    language     text NOT NULL,
    status       text NOT NULL CHECK (status IN ('authentic', 'official', 'translation', 'automatic')),
    wording_id   bigint NOT NULL REFERENCES wording(id),
    source       text,
    PRIMARY KEY (assertion_id, language)
);
CREATE INDEX expression_wording_idx ON expression (wording_id);

CREATE TABLE assertion_compile_state (
    text_uid          text PRIMARY KEY,
    journal_code      text NOT NULL REFERENCES journal(code),
    fingerprint       bigint NOT NULL,
    compiled_at       timestamptz NOT NULL DEFAULT now(),
    versions          integer NOT NULL,
    assertions        integer NOT NULL,
    never_applicable  integer NOT NULL,
    diagnostics       jsonb
);
CREATE INDEX assertion_compile_state_journal_idx ON assertion_compile_state (journal_code);

CREATE VIEW assertion_by_place AS
SELECT a.id, a.text_uid, a.num_key, p.place, a.starts_on, a.ends_on,
       a.start_origin, a.end_origin, a.version_start
FROM assertion a, unnest(a.places) AS p(place);

UPDATE facet_value SET parent_uid = 'geo:FR', label = 'Alsace-Moselle, droit local' WHERE uid = 'geo:FR-ALS';
UPDATE facet_value SET label = 'France métropolitaine, droit général' WHERE uid = 'geo:FR-M';

INSERT INTO facet_value (uid, facet, label, abbr, parent_uid, sort) VALUES
    ('geo:CH-AG', 'geo', 'Argovie',                   'AG', 'geo:CH', 200),
    ('geo:CH-AI', 'geo', 'Appenzell Rhodes-Intérieures', 'AI', 'geo:CH', 201),
    ('geo:CH-AR', 'geo', 'Appenzell Rhodes-Extérieures', 'AR', 'geo:CH', 202),
    ('geo:CH-BE', 'geo', 'Berne',                     'BE', 'geo:CH', 203),
    ('geo:CH-BL', 'geo', 'Bâle-Campagne',             'BL', 'geo:CH', 204),
    ('geo:CH-BS', 'geo', 'Bâle-Ville',                'BS', 'geo:CH', 205),
    ('geo:CH-FR', 'geo', 'Fribourg',                  'FR', 'geo:CH', 206),
    ('geo:CH-GE', 'geo', 'Genève',                    'GE', 'geo:CH', 207),
    ('geo:CH-GL', 'geo', 'Glaris',                    'GL', 'geo:CH', 208),
    ('geo:CH-GR', 'geo', 'Grisons',                   'GR', 'geo:CH', 209),
    ('geo:CH-JU', 'geo', 'Jura',                      'JU', 'geo:CH', 210),
    ('geo:CH-LU', 'geo', 'Lucerne',                   'LU', 'geo:CH', 211),
    ('geo:CH-NE', 'geo', 'Neuchâtel',                 'NE', 'geo:CH', 212),
    ('geo:CH-NW', 'geo', 'Nidwald',                   'NW', 'geo:CH', 213),
    ('geo:CH-OW', 'geo', 'Obwald',                    'OW', 'geo:CH', 214),
    ('geo:CH-SG', 'geo', 'Saint-Gall',                'SG', 'geo:CH', 215),
    ('geo:CH-SH', 'geo', 'Schaffhouse',               'SH', 'geo:CH', 216),
    ('geo:CH-SO', 'geo', 'Soleure',                   'SO', 'geo:CH', 217),
    ('geo:CH-SZ', 'geo', 'Schwytz',                   'SZ', 'geo:CH', 218),
    ('geo:CH-TG', 'geo', 'Thurgovie',                 'TG', 'geo:CH', 219),
    ('geo:CH-TI', 'geo', 'Tessin',                    'TI', 'geo:CH', 220),
    ('geo:CH-UR', 'geo', 'Uri',                       'UR', 'geo:CH', 221),
    ('geo:CH-VD', 'geo', 'Vaud',                      'VD', 'geo:CH', 222),
    ('geo:CH-VS', 'geo', 'Valais',                    'VS', 'geo:CH', 223),
    ('geo:CH-ZG', 'geo', 'Zoug',                      'ZG', 'geo:CH', 224),
    ('geo:CH-ZH', 'geo', 'Zurich',                    'ZH', 'geo:CH', 225),
    ('geo:BE-BRU', 'geo', 'Région de Bruxelles-Capitale',       NULL, 'geo:BE', 230),
    ('geo:BE-VLG', 'geo', 'Région flamande',                    NULL, 'geo:BE', 231),
    ('geo:BE-WAL', 'geo', 'Région wallonne',                    NULL, 'geo:BE', 232),
    ('geo:BE-CFR', 'geo', 'Communauté française',               NULL, 'geo:BE', 233),
    ('geo:BE-CNL', 'geo', 'Communauté flamande',                NULL, 'geo:BE', 234),
    ('geo:BE-CDE', 'geo', 'Communauté germanophone',            NULL, 'geo:BE', 235),
    ('geo:CY', 'geo', 'Chypre',     NULL, NULL, 160),
    ('geo:CZ', 'geo', 'Tchéquie',   NULL, NULL, 161),
    ('geo:DK', 'geo', 'Danemark',   NULL, NULL, 162),
    ('geo:EE', 'geo', 'Estonie',    NULL, NULL, 163),
    ('geo:FI', 'geo', 'Finlande',   NULL, NULL, 164),
    ('geo:HR', 'geo', 'Croatie',    NULL, NULL, 165),
    ('geo:IE', 'geo', 'Irlande',    NULL, NULL, 166),
    ('geo:LT', 'geo', 'Lituanie',   NULL, NULL, 167),
    ('geo:LV', 'geo', 'Lettonie',   NULL, NULL, 168),
    ('geo:MT', 'geo', 'Malte',      NULL, NULL, 169),
    ('geo:SE', 'geo', 'Suède',      NULL, NULL, 170),
    ('geo:SI', 'geo', 'Slovénie',   NULL, NULL, 171),
    ('geo:SK', 'geo', 'Slovaquie',  NULL, NULL, 172),
    ('geo:GB', 'geo', 'Royaume-Uni', NULL, NULL, 173);

-- 0302_users_mcp_connected_at.sql
ALTER TABLE users ADD COLUMN mcp_connected_at timestamptz;

UPDATE users u
   SET mcp_connected_at = least(
           (SELECT min(t.created_at) FROM mcp_tokens t WHERE t.user_id = u.sub),
           (SELECT min(h.created_at) FROM user_search_history h
             WHERE h.user_sub = u.sub AND h.source = 'mcp'))
 WHERE EXISTS (SELECT 1 FROM mcp_tokens t WHERE t.user_id = u.sub)
    OR EXISTS (SELECT 1 FROM user_search_history h
                WHERE h.user_sub = u.sub AND h.source = 'mcp');

-- 0303_assertion_text_uid_foreign_keys.sql
ALTER TABLE assertion
    ADD CONSTRAINT assertion_text_fk FOREIGN KEY (text_uid)
    REFERENCES legal_text(text_uid) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE assertion_compile_state
    ADD CONSTRAINT assertion_compile_state_text_fk FOREIGN KEY (text_uid)
    REFERENCES legal_text(text_uid) ON UPDATE CASCADE ON DELETE CASCADE;

CREATE INDEX assertion_version_idx ON assertion (text_uid, num_key, version_start);

ALTER TABLE expression DROP COLUMN source;
DROP INDEX assertion_compile_state_journal_idx;

UPDATE facet_value SET label = 'France' WHERE uid = 'geo:FR';

-- 0304_quantum_observation_composante_tranche.sql
ALTER TABLE quantum_observation
    DROP CONSTRAINT quantum_observation_pkey,
    ADD COLUMN composante_ord smallint NOT NULL DEFAULT -1,
    ADD COLUMN tranche_ord    smallint NOT NULL DEFAULT -1,
    ADD COLUMN nature_aide    text,
    ADD COLUMN mode_emploi    text,
    ADD COLUMN specialisee    boolean,
    ADD COLUMN nuit           boolean,
    ADD PRIMARY KEY (decision_id, victime_ord, poste_ord, composante_ord, tranche_ord);

-- 0305_text_place_coverage_table.sql
CREATE TABLE text_place_coverage (
    text_uid text NOT NULL REFERENCES legal_text (text_uid) ON UPDATE CASCADE ON DELETE CASCADE,
    place text NOT NULL,
    starts_on date NOT NULL,
    ends_on date,
    CONSTRAINT text_place_coverage_window CHECK (ends_on IS NULL OR ends_on > starts_on)
);

CREATE INDEX text_place_coverage_place_idx ON text_place_coverage (place, starts_on, ends_on);
CREATE INDEX text_place_coverage_text_idx ON text_place_coverage (text_uid);

ALTER TABLE assertion_compile_state ADD COLUMN nature text;

-- 0306_text_place_coverage_journal_nature.sql
ALTER TABLE text_place_coverage ADD COLUMN journal_code text;
ALTER TABLE text_place_coverage ADD COLUMN nature text;

CREATE INDEX text_place_coverage_corpus_idx
    ON text_place_coverage (place, journal_code, nature, starts_on, ends_on);
DROP INDEX text_place_coverage_place_idx;

-- 0307_llm_credential_table.sql
CREATE TABLE llm_credential (
    fingerprint    text PRIMARY KEY,
    provider       text NOT NULL,
    purpose        text NOT NULL,
    label          text NOT NULL,
    kind           text NOT NULL CHECK (kind IN ('api_key', 'oauth')),
    secret         jsonb NOT NULL,
    expires_at     timestamptz,
    disabled_until timestamptz,
    last_status    smallint,
    marked_by      text,
    rotated_at     timestamptz NOT NULL DEFAULT now(),
    created_at     timestamptz NOT NULL DEFAULT now()
);

COMMENT ON COLUMN llm_credential.fingerprint IS 'xxh3-64 hex of the secret, the same one mistral_key_status keys on';
COMMENT ON COLUMN llm_credential.secret IS 'api_key: {"key"}; oauth: {"access_token","refresh_token","id_token","account_id"}';
COMMENT ON COLUMN llm_credential.rotated_at IS 'concurrency token: a refresh lands only if it cites the value it read';

CREATE INDEX llm_credential_pool ON llm_credential (provider, purpose);

-- 0308_llm_credential_label_to_tenant.sql
ALTER TABLE llm_credential RENAME COLUMN label TO tenant;

COMMENT ON COLUMN llm_credential.tenant IS 'quota boundary the key belongs to: Mistral organisation, Google project, OpenAI account';

-- 0309_llm_credential_account_column.sql
ALTER TABLE llm_credential ADD COLUMN account text;

COMMENT ON COLUMN llm_credential.account IS 'console login that holds the tenant: Google account, Mistral account, OpenAI account';

-- 0310_llm_credential_schema_comptes.sql
ALTER TABLE llm_credential SET SCHEMA comptes;

-- 0311_drop_mistral_key_status.sql
DROP TABLE mistral_key_status;

-- 0312_legal_text_slug_jurisdiction_unique.sql
-- lj:sans-transaction
CREATE UNIQUE INDEX CONCURRENTLY idx_legal_text_slug_jurisdiction
ON legal_text (slug, jurisdiction)
WHERE slug IS NOT NULL;

-- 0313_annonce_metiers_array.sql
ALTER TABLE annonce ADD COLUMN metiers text[] NOT NULL DEFAULT '{}';

CREATE INDEX annonce_metiers_gin ON annonce USING gin (metiers);

INSERT INTO facet_value (uid, facet, label, sort) VALUES
  ('metier:agriculture', 'metier', 'Agriculture, forêt et espaces naturels', 1),
  ('metier:amiante', 'metier', 'Désamiantage et dépollution', 2),
  ('metier:ascenseur', 'metier', 'Ascenseurs et levage', 3),
  ('metier:assurance', 'metier', 'Assurance', 4),
  ('metier:audiovisuel', 'metier', 'Culture, événementiel et audiovisuel', 5),
  ('metier:batiment-tce', 'metier', 'Bâtiment tous corps d''état', 6),
  ('metier:charpente-couverture', 'metier', 'Charpente et couverture', 7),
  ('metier:communication', 'metier', 'Communication, impression et édition', 8),
  ('metier:conseil', 'metier', 'Conseil, audit et finance', 9),
  ('metier:dechets', 'metier', 'Déchets et propreté urbaine', 10),
  ('metier:eau', 'metier', 'Eau et assainissement', 11),
  ('metier:electricite', 'metier', 'Électricité et courants faibles', 12),
  ('metier:energie', 'metier', 'Énergie et fluides', 13),
  ('metier:espaces-verts', 'metier', 'Espaces verts et paysage', 14),
  ('metier:etudes', 'metier', 'Maîtrise d''oeuvre, études et ingénierie', 15),
  ('metier:finitions', 'metier', 'Second oeuvre et finitions', 16),
  ('metier:formation', 'metier', 'Formation et ressources humaines', 17),
  ('metier:genie-civil', 'metier', 'Génie civil et ouvrages d''art', 18),
  ('metier:genie-climatique', 'metier', 'Génie climatique et plomberie', 19),
  ('metier:gros-oeuvre', 'metier', 'Gros oeuvre, maçonnerie et patrimoine', 20),
  ('metier:habillement', 'metier', 'Textile et habillement', 21),
  ('metier:industrie', 'metier', 'Industrie, matières et matériaux', 22),
  ('metier:informatique', 'metier', 'Informatique et télécommunications', 23),
  ('metier:maintenance', 'metier', 'Exploitation et maintenance', 24),
  ('metier:menuiserie', 'metier', 'Menuiserie et fermetures', 25),
  ('metier:metallerie', 'metier', 'Métallerie et serrurerie', 26),
  ('metier:mobilier-equipement', 'metier', 'Mobilier et équipement', 27),
  ('metier:nettoyage', 'metier', 'Nettoyage et hygiène', 28),
  ('metier:restauration', 'metier', 'Restauration et alimentation', 29),
  ('metier:sante', 'metier', 'Santé, médical et laboratoire', 30),
  ('metier:securite', 'metier', 'Sécurité, gardiennage et incendie', 31),
  ('metier:sport-loisirs', 'metier', 'Sport, loisirs et jeux', 32),
  ('metier:terrassement', 'metier', 'Terrassement et démolition', 33),
  ('metier:transport', 'metier', 'Transport et véhicules', 34),
  ('metier:vrd', 'metier', 'Voirie et réseaux divers', 35);

-- 0314_drop_legal_text_slug_index.sql
-- lj:sans-transaction
DROP INDEX CONCURRENTLY idx_legal_text_slug;

-- 0315_jurisdiction_place_column.sql
ALTER TABLE jurisdiction ADD COLUMN place text;

UPDATE jurisdiction j
SET place = v.place
FROM (VALUES
    ('basse-terre', 'geo:GP'),
    ('pointe-à-pitre', 'geo:GP'),
    ('guadeloupe', 'geo:GP'),
    ('cayenne', 'geo:GF'),
    ('guyane', 'geo:GF'),
    ('fort-de-france', 'geo:MQ'),
    ('martinique', 'geo:MQ'),
    ('saint-denis de la réunion', 'geo:RE'),
    ('saint-pierre de la réunion', 'geo:RE'),
    ('la réunion', 'geo:RE'),
    ('mayotte', 'geo:YT'),
    ('nouméa', 'geo:NC'),
    ('nouvelle-calédonie', 'geo:NC'),
    ('papeete', 'geo:PF'),
    ('polynésie française', 'geo:PF'),
    ('saint-barthélemy', 'geo:BL'),
    ('saint-martin', 'geo:MF')
) AS v(city, place)
WHERE lower(j.city) = v.city;

-- 0316_llm_credential_priority_column.sql
ALTER TABLE comptes.llm_credential ADD COLUMN priority SMALLINT NOT NULL DEFAULT 0;

COMMENT ON COLUMN comptes.llm_credential.priority IS
    'service rank inside the pool, lowest served first; ties fall back to what the door announces';

-- 0317_pending_target_court_docket_date.sql
ALTER TABLE decision_links
    ADD COLUMN target_court  text,
    ADD COLUMN target_docket text,
    ADD COLUMN target_date   date;

ALTER TABLE case_citation
    ADD COLUMN target_court  text,
    ADD COLUMN target_docket text,
    ADD COLUMN target_date   date;

ALTER TABLE text_case_citation
    ADD COLUMN target_court  text,
    ADD COLUMN target_docket text,
    ADD COLUMN target_date   date;

ALTER TABLE decision_renvoi
    ADD COLUMN target_court  text,
    ADD COLUMN target_docket text;

ALTER TABLE decision_links ALTER COLUMN target_ref DROP NOT NULL;
ALTER TABLE case_citation ALTER COLUMN target_ref DROP NOT NULL;
ALTER TABLE text_case_citation ALTER COLUMN target_ref DROP NOT NULL;

ALTER TABLE decision_renvoi DROP CONSTRAINT decision_renvoi_pkey;
ALTER TABLE decision_renvoi ALTER COLUMN target_ref DROP NOT NULL;

COMMENT ON COLUMN decision_links.target_court IS
    'jurisdiction.code of the target, NULL when the gazetteer does not hold the name the source wrote';
COMMENT ON COLUMN decision_links.target_docket IS
    'docket as written; folding to a key belongs to the join, through lj_docket_keys';

CREATE INDEX decision_links_pending_addr_idx
    ON decision_links (target_court, target_date)
    WHERE target_decision_id IS NULL;
CREATE INDEX case_citation_pending_addr_idx
    ON case_citation (target_court, target_date)
    WHERE target_decision_id IS NULL;
CREATE INDEX text_case_citation_pending_addr_idx
    ON text_case_citation (target_court, target_date)
    WHERE target_decision_id IS NULL;
CREATE INDEX decision_renvoi_pending_addr_idx
    ON decision_renvoi (target_court, target_date)
    WHERE target_decision_id IS NULL;

CREATE INDEX decision_links_pending_docket_idx
    ON decision_links USING gin (lj_docket_keys(ARRAY[target_docket]))
    WHERE target_decision_id IS NULL AND target_docket IS NOT NULL;
CREATE INDEX case_citation_pending_docket_idx
    ON case_citation USING gin (lj_docket_keys(ARRAY[target_docket]))
    WHERE target_decision_id IS NULL AND target_docket IS NOT NULL;
CREATE INDEX text_case_citation_pending_docket_idx
    ON text_case_citation USING gin (lj_docket_keys(ARRAY[target_docket]))
    WHERE target_decision_id IS NULL AND target_docket IS NOT NULL;

-- 0318_llm_credential_purpose_nullable.sql
ALTER TABLE comptes.llm_credential ALTER COLUMN purpose DROP NOT NULL;

DROP INDEX comptes.llm_credential_pool;

CREATE INDEX llm_credential_pool ON comptes.llm_credential (provider);

-- 0319_drop_llm_credential_purpose.sql
ALTER TABLE comptes.llm_credential DROP COLUMN purpose;

-- 0320_users_browser_languages_country.sql
ALTER TABLE users
    ADD COLUMN browser_languages text[],
    ADD COLUMN country text;

UPDATE users SET browser_languages = ARRAY[browser_locale] WHERE browser_locale IS NOT NULL;

-- 0321_lj_docket_search_keys_function.sql
CREATE OR REPLACE FUNCTION lj_docket_search_keys(nums text[]) RETURNS text[]
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT AS $$
  SELECT array_agg(DISTINCT k ORDER BY k)
    FROM (SELECT k FROM unnest(lj_docket_keys(nums)) AS k
          UNION ALL
          SELECT substring(k FROM '^[a-z]+([0-9]+)$') FROM unnest(lj_docket_keys(nums)) AS k) s
   WHERE k IS NOT NULL
$$;

-- 0322_lj_docket_search_keys_schema_qualify.sql
CREATE OR REPLACE FUNCTION lj_docket_search_keys(nums text[]) RETURNS text[]
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT AS $$
  SELECT array_agg(DISTINCT k ORDER BY k)
    FROM (SELECT k FROM unnest(public.lj_docket_keys(nums)) AS k
          UNION ALL
          SELECT substring(k FROM '^[a-z]+([0-9]+)$') FROM unnest(public.lj_docket_keys(nums)) AS k) s
   WHERE k IS NOT NULL
$$;

-- 0323_decisions_bm25_v8_docket_search.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY decisions_bm25_v8 ON public.decisions USING bm25 (id, full_text, search_title, ((jurisdiction_type)::pdb.literal), ((legal_instruments)::pdb.literal), ((legal_article_composite)::pdb.literal), ((publication_codes)::pdb.literal), ((jurisdiction_code)::pdb.literal), ((seat_paths)::pdb.literal), ((legal_domain_uid)::pdb.literal), ((solution_uid)::pdb.literal), ((office_uid)::pdb.literal), ((procedure_uid)::pdb.literal), ((lj_docket_keys(docket_numbers))::pdb.literal('alias=docket_keys')), ((lj_docket_search_keys(docket_numbers))::pdb.literal('alias=docket_search')), ((ecli)::pdb.literal_normalized('alias=ecli')), date_lecture, deleted_at) WITH (key_field=id, text_fields='{"full_text": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}, "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}}');

-- 0324_decisions_bm25_swap_docket_search.sql
DROP INDEX decisions_bm25;

ALTER INDEX decisions_bm25_v8 RENAME TO decisions_bm25;

-- 0325_lj_fold_function.sql
CREATE OR REPLACE FUNCTION lj_fold(text) RETURNS text
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT
AS $fn$
SELECT btrim(regexp_replace(
    lower(translate(
        replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace($1, U&'\00C6', 'ae'), U&'\00DE', 'th'), U&'\00DF', 'ss'), U&'\00E6', 'ae'), U&'\00FE', 'th'), U&'\0132', 'ij'), U&'\0133', 'ij'), U&'\0149', '''n'), U&'\0152', 'oe'), U&'\0153', 'oe'), U&'\FB00', 'ff'), U&'\FB01', 'fi'), U&'\FB02', 'fl'), U&'\FB03', 'ffi'), U&'\FB04', 'ffl'), U&'\FB05', 'st'), U&'\FB06', 'st'),
        U&'\00C0\00C1\00C2\00C3\00C4\00C5\00C7\00C8\00C9\00CA\00CB\00CC\00CD\00CE\00CF\00D0\00D1\00D2\00D3\00D4\00D5\00D6\00D8\00D9\00DA\00DB\00DC\00DD\00E0\00E1\00E2\00E3\00E4\00E5\00E7\00E8\00E9\00EA\00EB\00EC\00ED\00EE\00EF\00F0\00F1\00F2\00F3\00F4\00F5\00F6\00F8\00F9\00FA\00FB\00FC\00FD\00FF\0100\0101\0102\0103\0104\0105\0106\0107\0108\0109\010A\010B\010C\010D\010E\010F\0110\0111\0112\0113\0114\0115\0116\0117\0118\0119\011A\011B\011C\011D\011E\011F\0120\0121\0122\0123\0124\0125\0126\0127\0128\0129\012A\012B\012C\012D\012E\012F\0130\0131\0134\0135\0136\0137\0138\0139\013A\013B\013C\013D\013E\013F\0140\0141\0142\0143\0144\0145\0146\0147\0148\014A\014B\014C\014D\014E\014F\0150\0151\0154\0155\0156\0157\0158\0159\015A\015B\015C\015D\015E\015F\0160\0161\0162\0163\0164\0165\0166\0167\0168\0169\016A\016B\016C\016D\016E\016F\0170\0171\0172\0173\0174\0175\0176\0177\0178\0179\017A\017B\017C\017D\017E\017F\2018\2019\02BC\00B4\201C\201D\201E\00AB\00BB\2010\2011\2012\2013\2014\2015\2212\0009\000A\000B\000C\000D\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000\00AD\200B\200C\200D\200E\200F\202A\202B\202C\202D\202E\2060\FEFF\0300\0301\0302\0303\0304\0305\0306\0307\0308\0309\030A\030B\030C\030D\030E\030F\0310\0311\0312\0313\0314\0315\0316\0317\0318\0319\031A\031B\031C\031D\031E\031F\0320\0321\0322\0323\0324\0325\0326\0327\0328\0329\032A\032B\032C\032D\032E\032F\0330\0331\0332\0333\0334\0335\0336\0337\0338\0339\033A\033B\033C\033D\033E\033F\0340\0341\0342\0343\0344\0345\0346\0347\0348\0349\034A\034B\034C\034D\034E\034F\0350\0351\0352\0353\0354\0355\0356\0357\0358\0359\035A\035B\035C\035D\035E\035F\0360\0361\0362\0363\0364\0365\0366\0367\0368\0369\036A\036B\036C\036D\036E\036F',
        'aaaaaaceeeeiiiidnoooooouuuuyaaaaaaceeeeiiiidnoooooouuuuyyaaaaaaccccccccddddeeeeeeeeeegggggggghhhhiiiiiiiiiijjkkkllllllllllnnnnnnnnoooooorrrrrrssssssssttttttuuuuuuuuuuuuwwyyyzzzzzzs''''''''"""""-------                        ')),
    ' +', ' ', 'g'))
$fn$;

-- 0326_reindex_entity_annuaire_idx.sql
-- lj:sans-transaction
REINDEX INDEX CONCURRENTLY entity_annuaire_idx;

-- 0327_reindex_entity_prefix_idx.sql
-- lj:sans-transaction
REINDEX INDEX CONCURRENTLY entity_prefix_idx;

-- 0328_reindex_entity_prefix_contentieux_idx.sql
-- lj:sans-transaction
REINDEX INDEX CONCURRENTLY entity_prefix_contentieux_idx;

-- 0329_reindex_entity_folded_all_idx.sql
-- lj:sans-transaction
REINDEX INDEX CONCURRENTLY entity_folded_all_idx;

-- 0330_assertion_rule_and_coverage_rule.sql
ALTER TABLE assertion ADD COLUMN rule text;
ALTER TABLE assertion DROP CONSTRAINT assertion_places_check;

ALTER TABLE assertion ADD CONSTRAINT assertion_rule_or_places
    CHECK ((rule IS NULL) = (cardinality(places) > 0)) NOT VALID;

CREATE TABLE coverage_rule (
    rule text NOT NULL,
    place text NOT NULL,
    since date,
    until date,
    since_source text NOT NULL,
    until_source text NOT NULL,
    CONSTRAINT coverage_rule_window CHECK (since IS NULL OR until IS NULL OR until > since)
);
CREATE INDEX coverage_rule_rule_idx ON coverage_rule (rule);

ALTER TABLE text_place_coverage ADD COLUMN rule text;
ALTER TABLE text_place_coverage ALTER COLUMN place DROP NOT NULL;
ALTER TABLE text_place_coverage ADD CONSTRAINT text_place_coverage_place_or_rule
    CHECK ((place IS NULL) <> (rule IS NULL));
CREATE INDEX text_place_coverage_rule_idx
    ON text_place_coverage (rule, journal_code, nature, starts_on, ends_on)
    WHERE rule IS NOT NULL;

CREATE OR REPLACE VIEW assertion_by_place AS
SELECT a.id, a.text_uid, a.num_key, p.place, a.starts_on, a.ends_on,
       a.start_origin, a.end_origin, a.version_start, a.start_source, a.end_source
FROM assertion a, unnest(a.places) AS p(place)
UNION ALL
SELECT k.id, k.text_uid, k.num_key, k.place, k.starts_on, k.ends_on,
       k.start_origin, k.end_origin, k.version_start, k.start_source, k.end_source
FROM (
    SELECT a.id, a.text_uid, a.num_key, r.place, a.version_start,
           CASE WHEN r.since > a.starts_on THEN r.since ELSE a.starts_on END AS starts_on,
           CASE WHEN r.since > a.starts_on THEN 'received' ELSE a.start_origin END AS start_origin,
           CASE WHEN r.since > a.starts_on THEN r.since_source ELSE a.start_source END AS start_source,
           CASE WHEN r.until < coalesce(a.ends_on, 'infinity') THEN r.until ELSE a.ends_on END AS ends_on,
           CASE WHEN r.until < coalesce(a.ends_on, 'infinity') THEN 'received' ELSE a.end_origin END AS end_origin,
           CASE WHEN r.until < coalesce(a.ends_on, 'infinity') THEN r.until_source ELSE a.end_source END AS end_source
    FROM assertion a
    JOIN coverage_rule r ON r.rule = a.rule
) k
WHERE k.ends_on IS NULL OR k.ends_on > k.starts_on;

UPDATE legal_text SET default_scope = 'geo:YT'
WHERE text_uid IN ('LEGITEXT000006072052', 'LEGITEXT000006071645', 'LEGITEXT000006071556',
                   'LEGITEXT000006069472', 'LEGITEXT000006074235');
UPDATE legal_text SET default_scope = 'geo:NC'
WHERE text_uid IN ('LEGITEXT000006070300', 'JORFTEXT000000280786', 'JORFTEXT000000647908');

-- 0331_drop_users_browser_locale.sql
ALTER TABLE users DROP COLUMN browser_locale;

-- 0332_pending_docket_idx_fastupdate_off.sql
SELECT gin_clean_pending_list('decision_links_pending_docket_idx');
SELECT gin_clean_pending_list('case_citation_pending_docket_idx');
SELECT gin_clean_pending_list('text_case_citation_pending_docket_idx');

ALTER INDEX decision_links_pending_docket_idx SET (fastupdate = off);
ALTER INDEX case_citation_pending_docket_idx SET (fastupdate = off);
ALTER INDEX text_case_citation_pending_docket_idx SET (fastupdate = off);

SELECT gin_clean_pending_list('decision_links_pending_docket_idx');
SELECT gin_clean_pending_list('case_citation_pending_docket_idx');
SELECT gin_clean_pending_list('text_case_citation_pending_docket_idx');

-- 0333_decision_text.sql
CREATE TABLE decision_text (
    id                      BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    decision_id             BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    lang                    TEXT NOT NULL CHECK (lang ~ '^[a-z]{2}$'),
    text_status             TEXT NOT NULL CHECK (text_status IN ('authentic', 'official', 'translation', 'automatic')),
    lead                    BOOLEAN NOT NULL DEFAULT TRUE,
    source_uid              TEXT,
    full_text               TEXT NOT NULL,
    search_title            TEXT,
    summary                 TEXT,
    summary_engine          SMALLINT,
    summary_prompt_version  SMALLINT,
    jurisdiction_type       TEXT NOT NULL,
    jurisdiction_code       TEXT,
    seat_paths              TEXT[],
    legal_instruments       TEXT[],
    legal_article_composite TEXT[],
    publication_codes       TEXT[] NOT NULL DEFAULT '{}',
    legal_domain_uid        TEXT,
    solution_uid            TEXT,
    office_uid              TEXT,
    procedure_uid           TEXT,
    docket_numbers          TEXT[] NOT NULL DEFAULT '{}',
    ecli                    TEXT,
    date_lecture            DATE,
    deleted_at              TIMESTAMPTZ,
    UNIQUE (decision_id, lang)
);

ALTER TABLE decisions ADD COLUMN lang TEXT NOT NULL DEFAULT 'fr';

ALTER TABLE decision_chunks ADD COLUMN lang TEXT NOT NULL DEFAULT 'fr';

CREATE FUNCTION decision_text_mirror(p_ids BIGINT[]) RETURNS VOID LANGUAGE sql AS $$
    WITH src AS (
        SELECT d.*, s.source_uid, s.source,
               CASE WHEN s.source_uid IS NULL OR s.lang IS NULL OR s.lang = 'fra' THEN 'fr'
                    WHEN s.lang = 'eng' THEN 'en' END AS text_lang
        FROM decisions d
        LEFT JOIN LATERAL (
            SELECT ds.source_uid, ds.lang, ds.source FROM decision_sources ds
            WHERE ds.decision_id = d.id
            ORDER BY ds.source_rank DESC, (ds.lang = 'fra') IS TRUE DESC, ds.source_uid ASC
            LIMIT 1
        ) s ON TRUE
        WHERE d.id = ANY(p_ids) AND d.full_text IS NOT NULL
    ), dropped AS (
        DELETE FROM decision_text t
        WHERE t.decision_id = ANY(p_ids)
          AND NOT EXISTS (SELECT 1 FROM src WHERE src.id = t.decision_id AND src.text_lang = t.lang)
    )
    INSERT INTO decision_text (
        decision_id, lang, text_status, source_uid, full_text, search_title, summary, summary_engine, summary_prompt_version,
        jurisdiction_type, jurisdiction_code, seat_paths, legal_instruments, legal_article_composite,
        publication_codes, legal_domain_uid, solution_uid, office_uid, procedure_uid, docket_numbers,
        ecli, date_lecture, deleted_at)
    SELECT id, text_lang,
           CASE WHEN source = 'cjue' AND text_lang <> lang THEN 'official' ELSE 'authentic' END,
           source_uid, full_text, search_title, summary, summary_engine, summary_prompt_version,
           jurisdiction_type, jurisdiction_code, seat_paths, legal_instruments, legal_article_composite,
           publication_codes, legal_domain_uid, solution_uid, office_uid, procedure_uid, docket_numbers,
           ecli, date_lecture, deleted_at
    FROM src
    ON CONFLICT (decision_id, lang) DO UPDATE SET
        text_status = EXCLUDED.text_status,
        source_uid = EXCLUDED.source_uid,
        full_text = CASE WHEN decision_text.full_text IS DISTINCT FROM EXCLUDED.full_text
                         THEN EXCLUDED.full_text ELSE decision_text.full_text END,
        search_title = EXCLUDED.search_title,
        summary = EXCLUDED.summary,
        summary_engine = EXCLUDED.summary_engine,
        summary_prompt_version = EXCLUDED.summary_prompt_version,
        jurisdiction_type = EXCLUDED.jurisdiction_type,
        jurisdiction_code = EXCLUDED.jurisdiction_code,
        seat_paths = EXCLUDED.seat_paths,
        legal_instruments = EXCLUDED.legal_instruments,
        legal_article_composite = EXCLUDED.legal_article_composite,
        publication_codes = EXCLUDED.publication_codes,
        legal_domain_uid = EXCLUDED.legal_domain_uid,
        solution_uid = EXCLUDED.solution_uid,
        office_uid = EXCLUDED.office_uid,
        procedure_uid = EXCLUDED.procedure_uid,
        docket_numbers = EXCLUDED.docket_numbers,
        ecli = EXCLUDED.ecli,
        date_lecture = EXCLUDED.date_lecture,
        deleted_at = EXCLUDED.deleted_at;
$$;

CREATE FUNCTION decision_text_mirror_trigger() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME = 'decisions' THEN
        PERFORM decision_text_mirror(ARRAY[NEW.id]);
    ELSIF TG_OP = 'DELETE' THEN
        PERFORM decision_text_mirror(ARRAY[OLD.decision_id]);
    ELSIF TG_OP = 'UPDATE' THEN
        PERFORM decision_text_mirror(ARRAY[OLD.decision_id, NEW.decision_id]);
    ELSE
        PERFORM decision_text_mirror(ARRAY[NEW.decision_id]);
    END IF;
    RETURN NULL;
END $$;

CREATE CONSTRAINT TRIGGER decisions_mirror_text
    AFTER INSERT OR UPDATE OF full_text, search_title, summary, summary_engine, summary_prompt_version, lang,
        jurisdiction_type, jurisdiction_code, seat_paths, legal_instruments, legal_article_composite,
        publication_codes, legal_domain_uid, solution_uid, office_uid, procedure_uid, docket_numbers,
        ecli, date_lecture, deleted_at
    ON decisions DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source_rank, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0334_decision_text_bm25.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY decision_text_bm25 ON public.decision_text USING bm25 (id, decision_id, full_text, search_title, ((jurisdiction_type)::pdb.literal), ((legal_instruments)::pdb.literal), ((legal_article_composite)::pdb.literal), ((publication_codes)::pdb.literal), ((jurisdiction_code)::pdb.literal), ((seat_paths)::pdb.literal), ((legal_domain_uid)::pdb.literal), ((solution_uid)::pdb.literal), ((office_uid)::pdb.literal), ((procedure_uid)::pdb.literal), ((lj_docket_keys(docket_numbers))::pdb.literal('alias=docket_keys')), ((lj_docket_search_keys(docket_numbers))::pdb.literal('alias=docket_search')), ((ecli)::pdb.literal_normalized('alias=ecli')), date_lecture, deleted_at, lead) WITH (key_field=id, text_fields='{"full_text": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}, "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}}');

-- 0335_decision_text_writers.sql
CREATE OR REPLACE FUNCTION decision_text_put(p_id BIGINT, p_text TEXT) RETURNS VOID LANGUAGE plpgsql AS $$
DECLARE
    v_lang TEXT;
    v_status TEXT;
    v_source_uid TEXT;
BEGIN
    SELECT CASE WHEN s.source_uid IS NULL OR s.lang IS NULL OR s.lang = 'fra' THEN 'fr'
                WHEN s.lang = 'eng' THEN 'en' END,
           CASE WHEN s.source = 'cjue' AND (CASE WHEN s.lang = 'eng' THEN 'en' ELSE 'fr' END) <> d.lang
                THEN 'official' ELSE 'authentic' END,
           s.source_uid
      INTO v_lang, v_status, v_source_uid
      FROM decisions d
      LEFT JOIN LATERAL (
          SELECT ds.source_uid, ds.lang, ds.source FROM decision_sources ds
          WHERE ds.decision_id = d.id
          ORDER BY ds.source_rank DESC, (ds.lang = 'fra') IS TRUE DESC, ds.source_uid ASC
          LIMIT 1
      ) s ON TRUE
     WHERE d.id = p_id;
    IF (SELECT count(*) FROM (SELECT 1 FROM decision_text WHERE decision_id = p_id LIMIT 2) n) = 1 THEN
        UPDATE decision_text SET lang = v_lang, text_status = v_status, source_uid = v_source_uid, full_text = p_text
         WHERE decision_id = p_id
           AND (lang, text_status, source_uid, full_text) IS DISTINCT FROM (v_lang, v_status, v_source_uid, p_text);
        RETURN;
    END IF;
    INSERT INTO decision_text (
        decision_id, lang, text_status, source_uid, full_text, search_title,
        jurisdiction_type, jurisdiction_code, seat_paths, legal_instruments, legal_article_composite,
        publication_codes, legal_domain_uid, solution_uid, office_uid, procedure_uid, docket_numbers,
        ecli, date_lecture, deleted_at)
    SELECT d.id, v_lang, v_status, v_source_uid, p_text, d.search_title,
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
END $$;

CREATE OR REPLACE FUNCTION decision_text_mirror(p_ids BIGINT[]) RETURNS VOID LANGUAGE plpgsql AS $$
DECLARE
    v_id BIGINT;
    v_text TEXT;
BEGIN
    FOR v_id, v_text IN SELECT id, full_text FROM decisions WHERE id = ANY(p_ids) AND full_text IS NOT NULL LOOP
        PERFORM decision_text_put(v_id, v_text);
    END LOOP;
    UPDATE decision_text t SET
        summary = d.summary,
        summary_engine = d.summary_engine,
        summary_prompt_version = d.summary_prompt_version
      FROM decisions d
     WHERE d.id = ANY(p_ids) AND t.decision_id = d.id AND t.lead AND d.summary IS NOT NULL
       AND (t.summary, t.summary_engine, t.summary_prompt_version)
           IS DISTINCT FROM (d.summary, d.summary_engine, d.summary_prompt_version);
    UPDATE decision_text t SET
        search_title = d.search_title,
        jurisdiction_type = d.jurisdiction_type,
        jurisdiction_code = d.jurisdiction_code,
        seat_paths = d.seat_paths,
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
       AND (t.search_title, t.jurisdiction_type, t.jurisdiction_code, t.seat_paths, t.legal_instruments,
            t.legal_article_composite, t.publication_codes, t.legal_domain_uid, t.solution_uid, t.office_uid,
            t.procedure_uid, t.docket_numbers, t.ecli, t.date_lecture, t.deleted_at)
           IS DISTINCT FROM
           (d.search_title, d.jurisdiction_type, d.jurisdiction_code, d.seat_paths, d.legal_instruments,
            d.legal_article_composite, d.publication_codes, d.legal_domain_uid, d.solution_uid, d.office_uid,
            d.procedure_uid, d.docket_numbers, d.ecli, d.date_lecture, d.deleted_at);
    FOR v_id, v_text IN
        SELECT t.decision_id, t.full_text FROM decision_text t JOIN decisions d ON d.id = t.decision_id
         WHERE t.decision_id = ANY(p_ids) AND d.full_text IS NULL
           AND (SELECT count(*) FROM (SELECT 1 FROM decision_text x WHERE x.decision_id = t.decision_id LIMIT 2) n) = 1
    LOOP
        PERFORM decision_text_put(v_id, v_text);
    END LOOP;
END $$;

-- 0336_drop_decisions_bm25.sql
-- lj:sans-transaction
DROP INDEX CONCURRENTLY decisions_bm25;

-- 0337_drop_decisions_text_columns.sql
DROP TRIGGER decisions_mirror_text ON decisions;

CREATE OR REPLACE FUNCTION decision_text_mirror(p_ids BIGINT[]) RETURNS VOID LANGUAGE plpgsql AS $$
DECLARE
    v_id BIGINT;
    v_text TEXT;
BEGIN
    UPDATE decision_text t SET
        search_title = d.search_title,
        jurisdiction_type = d.jurisdiction_type,
        jurisdiction_code = d.jurisdiction_code,
        seat_paths = d.seat_paths,
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
       AND (t.search_title, t.jurisdiction_type, t.jurisdiction_code, t.seat_paths, t.legal_instruments,
            t.legal_article_composite, t.publication_codes, t.legal_domain_uid, t.solution_uid, t.office_uid,
            t.procedure_uid, t.docket_numbers, t.ecli, t.date_lecture, t.deleted_at)
           IS DISTINCT FROM
           (d.search_title, d.jurisdiction_type, d.jurisdiction_code, d.seat_paths, d.legal_instruments,
            d.legal_article_composite, d.publication_codes, d.legal_domain_uid, d.solution_uid, d.office_uid,
            d.procedure_uid, d.docket_numbers, d.ecli, d.date_lecture, d.deleted_at);
    FOR v_id, v_text IN
        SELECT t.decision_id, t.full_text FROM decision_text t
         WHERE t.decision_id = ANY(p_ids)
           AND (SELECT count(*) FROM (SELECT 1 FROM decision_text x WHERE x.decision_id = t.decision_id LIMIT 2) n) = 1
    LOOP
        PERFORM decision_text_put(v_id, v_text);
    END LOOP;
END $$;

ALTER TABLE decisions DROP COLUMN full_text, DROP COLUMN summary, DROP COLUMN summary_engine,
    DROP COLUMN summary_prompt_version;

CREATE CONSTRAINT TRIGGER decisions_mirror_text
    AFTER INSERT OR UPDATE OF search_title, lang,
        jurisdiction_type, jurisdiction_code, seat_paths, legal_instruments, legal_article_composite,
        publication_codes, legal_domain_uid, solution_uid, office_uid, procedure_uid, docket_numbers,
        ecli, date_lecture, deleted_at
    ON decisions DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

-- 0338_decision_text_per_provenance.sql

CREATE FUNCTION decision_text_put(p_id BIGINT, p_source_uid TEXT, p_lang TEXT, p_text TEXT) RETURNS VOID LANGUAGE plpgsql AS $$
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
    UPDATE decision_text t SET lead = (t.id = l.id)
      FROM (SELECT x.id FROM decision_text x LEFT JOIN decision_sources s ON s.source_uid = x.source_uid
             WHERE x.decision_id = p_id
             ORDER BY s.source_rank DESC NULLS LAST, (s.lang = 'fra') IS TRUE DESC, (s.lang = 'eng') IS TRUE DESC,
                      x.source_uid ASC
             LIMIT 1) l
     WHERE t.decision_id = p_id AND t.lead IS DISTINCT FROM (t.id = l.id);
END $$;

CREATE OR REPLACE FUNCTION decision_text_mirror(p_ids BIGINT[]) RETURNS VOID LANGUAGE sql AS $$
    UPDATE decision_text t SET
        search_title = d.search_title,
        jurisdiction_type = d.jurisdiction_type,
        jurisdiction_code = d.jurisdiction_code,
        seat_paths = d.seat_paths,
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
       AND (t.search_title, t.jurisdiction_type, t.jurisdiction_code, t.seat_paths, t.legal_instruments,
            t.legal_article_composite, t.publication_codes, t.legal_domain_uid, t.solution_uid, t.office_uid,
            t.procedure_uid, t.docket_numbers, t.ecli, t.date_lecture, t.deleted_at)
           IS DISTINCT FROM
           (d.search_title, d.jurisdiction_type, d.jurisdiction_code, d.seat_paths, d.legal_instruments,
            d.legal_article_composite, d.publication_codes, d.legal_domain_uid, d.solution_uid, d.office_uid,
            d.procedure_uid, d.docket_numbers, d.ecli, d.date_lecture, d.deleted_at);
$$;

