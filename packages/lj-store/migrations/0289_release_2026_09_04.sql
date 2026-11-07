-- 0208_decisions_public_id_check_not_valid.sql
SET LOCAL lock_timeout = '5s';

ALTER TABLE decisions
  ADD CONSTRAINT decisions_public_id_present CHECK (public_id IS NOT NULL) NOT VALID;

-- 0209_decisions_public_id_not_null.sql
SET LOCAL lock_timeout = '5s';

ALTER TABLE decisions VALIDATE CONSTRAINT decisions_public_id_present;

ALTER TABLE decisions ALTER COLUMN public_id SET NOT NULL;

ALTER TABLE decisions DROP CONSTRAINT decisions_public_id_present;

-- 0210_auth_password_sessions_email_tokens.sql
ALTER TABLE users
    ADD COLUMN password_hash      text,
    ADD COLUMN email_confirmed_at timestamptz,
    ADD COLUMN google_sub         text;

CREATE UNIQUE INDEX users_email_lower_uidx ON users (lower(email)) WHERE email IS NOT NULL;
CREATE UNIQUE INDEX users_google_sub_uidx ON users (google_sub) WHERE google_sub IS NOT NULL;

CREATE TABLE web_sessions (
    session_sha256      bytea PRIMARY KEY,
    user_sub            text NOT NULL REFERENCES users(sub) ON DELETE CASCADE,
    expires_at          timestamptz NOT NULL,
    absolute_expires_at timestamptz NOT NULL,
    created_at          timestamptz NOT NULL DEFAULT now(),
    last_used_at        timestamptz NOT NULL DEFAULT now(),
    CHECK (absolute_expires_at >= expires_at)
);
CREATE INDEX web_sessions_expires_at_idx ON web_sessions (expires_at);
CREATE INDEX web_sessions_user_sub_idx ON web_sessions (user_sub);

CREATE TABLE auth_email_tokens (
    token_sha256 bytea PRIMARY KEY,
    user_sub     text NOT NULL REFERENCES users(sub) ON DELETE CASCADE,
    purpose      text NOT NULL CHECK (purpose IN ('confirmation', 'recovery')),
    expires_at   timestamptz NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    used_at      timestamptz
);
CREATE INDEX auth_email_tokens_expires_at_idx ON auth_email_tokens (expires_at);
CREATE INDEX auth_email_tokens_user_purpose_idx ON auth_email_tokens (user_sub, purpose);

-- 0211_lj_docket_keys_function.sql
CREATE OR REPLACE FUNCTION lj_docket_keys(nums text[]) RETURNS text[]
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT AS $$
  SELECT array_agg(DISTINCT k ORDER BY k)
    FROM (SELECT lower(regexp_replace(n, '[^[:alnum:]]', '', 'g')) AS k
            FROM unnest(nums) AS n) s
   WHERE k <> ''
$$;

-- 0212_idx_decisions_docket_keys_gin.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY idx_decisions_docket_keys ON decisions
USING gin (lj_docket_keys(docket_numbers)) WHERE deleted_at IS NULL;

-- 0213_decisions_bm25_number_tokenizer.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY decisions_bm25_v2 ON decisions
USING bm25 (
  id,
  full_text,
  search_title,
  ((jurisdiction_type)::pdb.literal),
  ((legal_instruments)::pdb.literal),
  ((legal_article_composite)::pdb.literal),
  ((publication_codes)::pdb.literal),
  date_lecture
) WITH (key_field=id, text_fields='{"full_text": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}, "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}}');

-- 0214_decisions_bm25_swap_v2.sql
DROP INDEX decisions_bm25;

ALTER INDEX decisions_bm25_v2 RENAME TO decisions_bm25;

-- 0215_legal_text_slug_redirect.sql
CREATE TABLE legal_text_slug_redirect (
    ancien_slug text PRIMARY KEY,
    text_uid text NOT NULL REFERENCES legal_text(text_uid) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX legal_text_slug_redirect_uid_idx ON legal_text_slug_redirect (text_uid);

-- 0216_code_slugs_drop_artefacts.sql
WITH renames(ancien, nouveau) AS (VALUES

  ('code-general-des-impots-cgi',                 'code-general-des-impots'),
  ('code-general-des-impots-annexe-i-cgiani',     'code-general-des-impots-annexe-i'),
  ('code-general-des-impots-annexe-ii-cgianii',   'code-general-des-impots-annexe-ii'),
  ('code-general-des-impots-annexe-iii-cgianiii', 'code-general-des-impots-annexe-iii'),
  ('code-general-des-impots-annexe-iv-cgianiv',   'code-general-des-impots-annexe-iv'),

  ('code-de-commerce-2',          'code-de-commerce-ancien'),
  ('code-penal-2',                'code-penal-ancien'),
  ('code-de-la-route-2',          'code-de-la-route-ancien'),
  ('code-de-justice-militaire-2', 'code-de-justice-militaire-nouveau'),

  ('code-des-marches-publics-2', 'code-des-marches-publics-edition-1964'),
  ('code-des-marches-publics-3', 'code-des-marches-publics-edition-2001'),
  ('code-des-marches-publics-4', 'code-des-marches-publics-edition-2004')
),
cible AS (
  SELECT r.ancien, r.nouveau, t.text_uid
    FROM renames r JOIN legal_text t ON t.slug = r.ancien
),
trace AS (
  INSERT INTO legal_text_slug_redirect (ancien_slug, text_uid)
  SELECT ancien, text_uid FROM cible
  RETURNING text_uid
)
UPDATE legal_text t SET slug = c.nouveau
  FROM cible c WHERE t.text_uid = c.text_uid;

UPDATE legal_article a SET slug = t.slug
  FROM legal_text t
 WHERE t.text_uid = a.text_uid
   AND t.text_uid IN (SELECT text_uid FROM legal_text_slug_redirect)
   AND a.slug IS DISTINCT FROM t.slug;

-- 0217_user_identities_and_email_change.sql
CREATE TABLE user_identities (
    provider     text        NOT NULL,
    provider_sub text        NOT NULL,
    user_sub     text        NOT NULL REFERENCES users(sub) ON DELETE CASCADE,
    linked_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (provider, provider_sub)
);
CREATE INDEX user_identities_user_sub_idx ON user_identities (user_sub);

INSERT INTO user_identities (provider, provider_sub, user_sub)
SELECT 'google', google_sub, sub FROM users WHERE google_sub IS NOT NULL;

ALTER TABLE auth_email_tokens ADD COLUMN new_email text;
ALTER TABLE auth_email_tokens DROP CONSTRAINT auth_email_tokens_purpose_check;
ALTER TABLE auth_email_tokens ADD CONSTRAINT auth_email_tokens_purpose_check
    CHECK (purpose IN ('confirmation', 'recovery', 'email_change_ancienne', 'email_change_nouvelle'));

-- 0218_legal_article_bm25_int_tokenizer.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY legal_article_bm25_v2 ON legal_article
USING bm25 (
  id,
  search_title,
  texte,
  num,
  ((source)::pdb.literal),
  ((text_uid)::pdb.literal),
  ((status)::pdb.literal),
  ((jurisdiction)::pdb.literal),
  ((nature)::pdb.literal),
  ((slug)::pdb.literal),
  ((kind)::pdb.literal),
  searchable,
  date_debut,
  date_fin
) WITH (key_field=id, text_fields='{
    "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "texte":        {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "num":          {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "basic"}
  }');

-- 0219_legal_text_body_bm25_int_tokenizer.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY legal_text_body_bm25_v2 ON legal_text
USING bm25 (
  id,
  title,
  body,
  ((text_uid)::pdb.literal),
  ((nature)::pdb.literal),
  ((jurisdiction)::pdb.literal),
  ((slug)::pdb.literal),
  ((role)::pdb.literal),
  ((COALESCE(status, 'VIGUEUR'::text))::pdb.literal('alias=status_eff')),
  ((upper(nature))::pdb.literal('alias=nature_up')),
  ((lower(nature))::pdb.literal('alias=source')),
  (((body IS NOT NULL))::pdb.alias('alias=has_body'))
) WITH (key_field=id, text_fields='{
    "title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "body":  {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}
  }');

-- 0220_legal_article_usage_bm25_int_tokenizer.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY legal_article_usage_bm25_v2 ON legal_article_usage
USING bm25 (
  id,
  terms,
  ((text_uid)::pdb.literal),
  ((num_key)::pdb.literal)
) WITH (key_field=id, text_fields='{
    "terms": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}_-]+", "ascii_folding": true}, "record": "freq"}
  }');

-- 0221_legal_text_engine_bm25_swap.sql
DROP INDEX legal_article_bm25;
DROP INDEX legal_text_body_bm25;
DROP INDEX legal_article_usage_bm25;

ALTER INDEX legal_article_bm25_v2 RENAME TO legal_article_bm25;
ALTER INDEX legal_text_body_bm25_v2 RENAME TO legal_text_body_bm25;
ALTER INDEX legal_article_usage_bm25_v2 RENAME TO legal_article_usage_bm25;

-- 0222_drop_users_google_sub.sql
ALTER TABLE users DROP COLUMN google_sub;

-- 0223_legal_text_norm_browse_idx.sql
CREATE INDEX idx_legal_text_norm_browse ON legal_text (
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
) WHERE slug IS NOT NULL;

DROP INDEX idx_legal_text_norm_fond_browse;

-- 0224_jurisdiction_label_overseas_fix.sql
UPDATE jurisdiction SET label = 'Tribunal de première instance de Papeete'
 WHERE code = 'tj_papeete';
UPDATE jurisdiction SET label = 'Tribunal de première instance de Nouméa'
 WHERE code = 'tj_noumea';
UPDATE jurisdiction SET label = 'Tribunal mixte de commerce de Fort-de-France'
 WHERE code = 'tcom_fort_de_france';

-- 0225_jurisdiction_year_counts.sql
CREATE TABLE jurisdiction_year_counts (
    jurisdiction_code text   NOT NULL,
    year              int,
    decision_count    bigint NOT NULL,
    CONSTRAINT jurisdiction_year_counts_key
        UNIQUE NULLS NOT DISTINCT (jurisdiction_code, year)
);

INSERT INTO jurisdiction_year_counts (jurisdiction_code, year, decision_count)
SELECT jurisdiction_code, extract(year FROM date_lecture)::int, count(*)
FROM decisions
WHERE deleted_at IS NULL
GROUP BY 1, 2;

-- 0226_idx_decisions_jur_code_date_v2.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY idx_decisions_jur_code_date_v2
ON decisions (jurisdiction_code, date_lecture DESC, id)
WHERE deleted_at IS NULL;

-- 0227_idx_decisions_jur_code_date_swap.sql
DROP INDEX idx_decisions_jur_code_date;

ALTER INDEX idx_decisions_jur_code_date_v2
  RENAME TO idx_decisions_jur_code_date;

-- 0228_legal_article_search_title_star.sql
DROP INDEX IF EXISTS legal_article_bm25;

ALTER TABLE legal_article DROP COLUMN IF EXISTS search_title;

ALTER TABLE legal_article ADD COLUMN search_title TEXT GENERATED ALWAYS AS (
  COALESCE(code_title, '')
  || ', ' || num
  || CASE WHEN num LIKE '%*%' THEN ', ' || num_key ELSE '' END
  || CASE WHEN title_path IS NOT NULL AND title_path <> ''
       THEN ', ' || regexp_replace(title_path, '^.*>\s*', '')
       ELSE '' END
) STORED;

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

ANALYZE legal_article;

-- 0229_facet_value_hint_and_publication.sql
ALTER TABLE facet_value ADD COLUMN hint TEXT;

INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('publication:PUBLIE_RAPPORT', 'publication', 'Publié au rapport',             2),
    ('publication:LETTRE_CHAMBRE', 'publication', 'Publié aux lettres de chambre', 3),
    ('publication:COMMUNIQUE',     'publication', 'Communiqué',                    4);

UPDATE facet_value SET sort = 1 WHERE uid = 'publication:PUBLIE_BULLETIN';
UPDATE facet_value SET sort = 5 WHERE uid = 'publication:INEDIT_BULLETIN';
UPDATE facet_value SET sort = 6 WHERE uid = 'publication:PUBLIE_LEBON';
UPDATE facet_value SET sort = 7 WHERE uid = 'publication:MENTIONNE_LEBON';
UPDATE facet_value SET sort = 8 WHERE uid = 'publication:INEDIT_LEBON';

UPDATE facet_value SET hint = 'Couvre les codes source B, C+ et R.'
WHERE uid = 'publication:MENTIONNE_LEBON';
UPDATE facet_value SET hint = 'Couvre les codes source C, D et Z.'
WHERE uid = 'publication:INEDIT_LEBON';
UPDATE facet_value SET hint = 'Première, deuxième et troisième chambres civiles.'
WHERE uid = 'chamber:CIVILE';
UPDATE facet_value SET hint = 'Chambre commerciale, financière et économique de la Cour de cassation, et chambres commerciales des cours d''appel et des tribunaux.'
WHERE uid = 'chamber:COMMERCIALE';

-- 0230_facet_value_publication_sort.sql
UPDATE facet_value SET sort = 1 WHERE uid = 'publication:PUBLIE_RAPPORT';
UPDATE facet_value SET sort = 2 WHERE uid = 'publication:PUBLIE_BULLETIN';

-- 0231_decision_facet_counts.sql
CREATE TABLE decision_facet_counts (
    dimension text   NOT NULL,
    value     text   NOT NULL,
    parent    text,
    n         bigint NOT NULL,
    CONSTRAINT decision_facet_counts_key
        UNIQUE NULLS NOT DISTINCT (dimension, value, parent)
);

INSERT INTO decision_facet_counts (dimension, value, parent, n)
    SELECT 'total'::text, ''::text, NULL::text, count(*)
    FROM decisions WHERE deleted_at IS NULL
UNION ALL
    SELECT 'jurisdiction_type', jurisdiction_type, NULL, count(*)
    FROM decisions WHERE deleted_at IS NULL
    GROUP BY jurisdiction_type
UNION ALL
    SELECT 'jurisdiction_code', jurisdiction_code, jurisdiction_type, count(*)
    FROM decisions WHERE deleted_at IS NULL AND jurisdiction_code IS NOT NULL
    GROUP BY jurisdiction_code, jurisdiction_type
UNION ALL
    SELECT 'chamber', chamber_uid, NULL, count(*)
    FROM decisions WHERE deleted_at IS NULL AND chamber_uid IS NOT NULL
    GROUP BY chamber_uid
UNION ALL
    SELECT 'office', office_uid, NULL, count(*)
    FROM decisions WHERE deleted_at IS NULL AND office_uid IS NOT NULL
    GROUP BY office_uid
UNION ALL
    SELECT 'legal_domain', legal_domain_uid, NULL, count(*)
    FROM decisions WHERE deleted_at IS NULL AND legal_domain_uid IS NOT NULL
    GROUP BY legal_domain_uid
UNION ALL
    SELECT 'solution', solution_uid, NULL, count(*)
    FROM decisions WHERE deleted_at IS NULL AND solution_uid IS NOT NULL
    GROUP BY solution_uid
UNION ALL
    SELECT 'year', extract(year FROM date_lecture)::int::text, NULL, count(*)
    FROM decisions WHERE deleted_at IS NULL AND date_lecture IS NOT NULL
    GROUP BY 2
UNION ALL
    SELECT 'publication_codes', array_to_string(publication_codes, ','), NULL, count(*)
    FROM decisions WHERE deleted_at IS NULL AND publication_codes <> '{}'
    GROUP BY 2;

-- 0232_idx_decisions_date_lecture_actives.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY idx_decisions_date_lecture_actives
ON decisions (date_lecture DESC NULLS LAST, id)
WHERE deleted_at IS NULL;

-- 0233_drop_decisions_publication_uid.sql
ALTER TABLE decisions DROP COLUMN publication_uid;

DELETE FROM facet_value WHERE uid = 'publication:AUTRE';
DELETE FROM facet_value WHERE facet = 'significance';

-- 0234_decisions_add_chamber_key.sql
ALTER TABLE decisions ADD COLUMN chamber_key TEXT;

CREATE INDEX idx_decisions_chamber_key ON decisions (jurisdiction_code, chamber_key)
  WHERE deleted_at IS NULL AND chamber_key IS NOT NULL;

-- 0235_decision_quantum_tables.sql
CREATE TABLE decision_quantum (
    decision_id           bigint      NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    pc_extract_version    smallint    NOT NULL,
    est_liquidation       boolean     NOT NULL,
    fait_generateur       text,
    regime                text,
    devise                text,
    bareme_capitalisation text,
    total_enonce_cents    bigint,
    total_enonce_base     text,
    atteste               boolean,
    raison_non_atteste    text,
    cases_confirmees      integer,
    corrections           jsonb,
    commentaire           text,
    lecture               jsonb,
    updated_at            timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (decision_id, pc_extract_version)
);

CREATE TABLE decision_quantum_victime (
    decision_id                bigint   NOT NULL,
    pc_extract_version         smallint NOT NULL,
    ord                        smallint NOT NULL,
    cle                        text     NOT NULL,
    role                       text     NOT NULL,
    lien                       text,
    sexe                       text,
    date_naissance             date,
    age_fait                   smallint,
    date_consolidation         date,
    age_consolidation          smallint,

    age_decision               smallint,
    taux_dfp_pct               double precision,
    partage_responsabilite_pct double precision,
    perte_de_chance_pct        double precision,
    profession                 text,

    mineur                     boolean,
    cohabitation               boolean,
    orphelin                   boolean,
    frequence_relations        text,
    postes_reduits             text[]   NOT NULL DEFAULT '{}',
    postes_reserves            text[]   NOT NULL DEFAULT '{}',
    total_alloue_cents          bigint,
    assiette_reduite_cents      bigint,
    provisions_deduites_cents   bigint,
    creance_tiers_payeurs_cents bigint,
    net_victime_cents           bigint,
    exhaustif                  boolean,
    ecart_liquidation          jsonb,
    champs_indecidables        text[],
    PRIMARY KEY (decision_id, pc_extract_version, ord),
    FOREIGN KEY (decision_id, pc_extract_version)
        REFERENCES decision_quantum (decision_id, pc_extract_version) ON DELETE CASCADE
);

CREATE TABLE decision_quantum_poste (
    decision_id              bigint   NOT NULL,
    pc_extract_version       smallint NOT NULL,
    victime_ord              smallint NOT NULL,
    ord                      smallint NOT NULL,
    poste                    text     NOT NULL,
    libelle_decision         text,
    montant_demande_cents    bigint,
    montant_offert_cents     bigint,
    montant_alloue_cents     bigint,
    part_victime_cents       bigint,
    part_tiers_payeurs_cents bigint,

    valeur_unitaire_cents    bigint,
    quantite                 double precision,
    unite                    text,
    taux_pct                 double precision,
    cotation                 double precision,
    char_start               integer,
    char_end                 integer,
    composantes              jsonb,
    PRIMARY KEY (decision_id, pc_extract_version, victime_ord, ord),
    FOREIGN KEY (decision_id, pc_extract_version, victime_ord)
        REFERENCES decision_quantum_victime (decision_id, pc_extract_version, ord) ON DELETE CASCADE
);

CREATE INDEX idx_decision_quantum_poste_version_poste
    ON decision_quantum_poste (pc_extract_version, poste);

-- 0236_decisions_add_matiere_uid.sql
ALTER TABLE decisions
  ADD COLUMN matiere_uid TEXT REFERENCES facet_value(uid);

INSERT INTO facet_value (uid, facet, label, abbr, parent_uid, sort, hint)
SELECT 'matiere:' || split_part(uid, ':', 2), 'matiere', label, abbr, NULL, sort, hint
  FROM facet_value
 WHERE facet = 'chamber';

-- 0237_quantum_observation_table.sql
DROP TABLE IF EXISTS decision_quantum_poste;
DROP TABLE IF EXISTS decision_quantum_victime;
DROP TABLE IF EXISTS decision_quantum;

CREATE TABLE decision_quantum (
    decision_id        bigint      NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    pc_extract_version smallint    NOT NULL,
    est_liquidation    boolean     NOT NULL,
    atteste            boolean,
    raison_non_atteste text,
    cases_confirmees   integer,
    quantum            jsonb       NOT NULL,
    updated_at         timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (decision_id, pc_extract_version)
);

CREATE TABLE quantum_observation (
    decision_id           bigint   NOT NULL,
    pc_extract_version    smallint NOT NULL,
    victime_ord           smallint NOT NULL,
    poste_ord             smallint NOT NULL,
    poste                 text     NOT NULL,

    axe                   text,
    hors_table            text,
    montant_cents         bigint,

    valeur_unitaire_cents bigint,
    quantite              double precision,
    unite                 text,
    taux_pct              double precision,
    cotation              double precision,
    categorie             text,
    taux_dfp_pct          double precision,
    age                   smallint,
    sexe                  text,
    role                  text,
    lien                  text,

    juridiction           text,
    annee                 smallint,
    PRIMARY KEY (decision_id, pc_extract_version, victime_ord, poste_ord),
    FOREIGN KEY (decision_id, pc_extract_version)
        REFERENCES decision_quantum (decision_id, pc_extract_version) ON DELETE CASCADE
);

CREATE INDEX idx_quantum_observation_poste
    ON quantum_observation (pc_extract_version, poste, axe);

-- 0238_alias_candidate_table.sql
CREATE TABLE alias_candidate (
    genre       text        NOT NULL,
    gisement    text        NOT NULL,
    alias_key   text        NOT NULL,
    alias_vu    text        NOT NULL,
    target_ref  text        NOT NULL,
    target_date date,
    support     integer     NOT NULL,
    calcule_le  timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX alias_candidate_cle
    ON alias_candidate (genre, gisement, alias_key, target_ref, target_date)
    NULLS NOT DISTINCT;

CREATE INDEX alias_candidate_revue ON alias_candidate (genre, support DESC);

-- 0239_drop_decisions_chamber_position_uid.sql
ALTER TABLE decisions
  DROP COLUMN chamber_position,
  DROP COLUMN chamber_uid;

DELETE FROM facet_value WHERE facet = 'chamber';

-- 0240_quantum_observation_add_deces.sql
ALTER TABLE quantum_observation ADD COLUMN deces boolean;

-- 0241_quantum_observation_add_comparabilite.sql
ALTER TABLE quantum_observation ADD COLUMN comparabilite jsonb;

-- 0242_quantum_observation_categorie_manque.sql
ALTER TABLE quantum_observation ADD COLUMN categorie_manque text;

-- 0243_chamber_table.sql
CREATE TABLE chamber (
  jurisdiction_code TEXT NOT NULL REFERENCES jurisdiction(code),
  chamber_key       TEXT NOT NULL,
  label             TEXT NOT NULL,
  PRIMARY KEY (jurisdiction_code, chamber_key)
);

-- 0244_decision_renvoi_table.sql
CREATE TABLE decision_renvoi (
    decision_id        int8 NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    renvoi_no          int2 NOT NULL,
    target_ref         text NOT NULL,
    target_date        date,
    nom                text,
    target_decision_id int8 REFERENCES decisions(id) ON DELETE SET NULL,
    PRIMARY KEY (decision_id, renvoi_no, target_ref)
);

CREATE INDEX decision_renvoi_cible_idx
    ON decision_renvoi (target_decision_id) WHERE target_decision_id IS NOT NULL;

-- 0245_decisions_bm25_v3_facet_axes.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY decisions_bm25_v3 ON decisions
USING bm25 (
  id,
  full_text,
  search_title,
  ((jurisdiction_type)::pdb.literal),
  ((legal_instruments)::pdb.literal),
  ((legal_article_composite)::pdb.literal),
  ((publication_codes)::pdb.literal),
  ((jurisdiction_code)::pdb.literal),
  ((chamber_key)::pdb.literal),
  ((matiere_uid)::pdb.literal),
  ((legal_domain_uid)::pdb.literal),
  ((solution_uid)::pdb.literal),
  ((office_uid)::pdb.literal),
  date_lecture,
  deleted_at
) WITH (key_field=id, text_fields='{"full_text": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}, "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}}');

-- 0246_decisions_bm25_v3_swap.sql
DROP INDEX decisions_bm25;

ALTER INDEX decisions_bm25_v3 RENAME TO decisions_bm25;

-- 0247_decision_sources_rank_rebuild.sql
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
            WHEN 'ariane-dce'   THEN 45
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-capp'    THEN 30
            ELSE 0
        END
    ) STORED;

INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('formation:ASSEMBLEE_PLENIERE', 'formation', 'Assemblée plénière', 13);

-- 0248_move_account_tables_to_comptes_schema.sql
DO $$
BEGIN
    EXECUTE format(
        'ALTER DATABASE %I SET search_path = public, paradedb, comptes',
        current_database()
    );
END
$$;

CREATE SCHEMA comptes;

ALTER TABLE users               SET SCHEMA comptes;
ALTER TABLE user_identities     SET SCHEMA comptes;
ALTER TABLE user_bookmarks      SET SCHEMA comptes;
ALTER TABLE user_search_history SET SCHEMA comptes;
ALTER TABLE user_decision_views SET SCHEMA comptes;
ALTER TABLE web_sessions        SET SCHEMA comptes;
ALTER TABLE auth_email_tokens   SET SCHEMA comptes;
ALTER TABLE mcp_clients         SET SCHEMA comptes;
ALTER TABLE mcp_tokens          SET SCHEMA comptes;
ALTER TABLE mcp_refresh_tokens  SET SCHEMA comptes;
ALTER TABLE mcp_auth_codes      SET SCHEMA comptes;

-- 0249_decisions_bm25_v4_procedure_uid.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY decisions_bm25_v4 ON decisions
USING bm25 (
  id,
  full_text,
  search_title,
  ((jurisdiction_type)::pdb.literal),
  ((legal_instruments)::pdb.literal),
  ((legal_article_composite)::pdb.literal),
  ((publication_codes)::pdb.literal),
  ((jurisdiction_code)::pdb.literal),
  ((chamber_key)::pdb.literal),
  ((matiere_uid)::pdb.literal),
  ((legal_domain_uid)::pdb.literal),
  ((solution_uid)::pdb.literal),
  ((office_uid)::pdb.literal),
  ((procedure_uid)::pdb.literal),
  date_lecture,
  deleted_at
) WITH (key_field=id, text_fields='{"full_text": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}, "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}}');

-- 0250_decisions_bm25_v4_swap.sql
DROP INDEX decisions_bm25;

ALTER INDEX decisions_bm25_v4 RENAME TO decisions_bm25;

-- 0251_decisions_add_case_name.sql
ALTER TABLE decisions
    ADD COLUMN case_name text,
    ADD COLUMN case_name_keys text[];

CREATE INDEX decisions_case_name_keys_idx ON decisions USING gin (case_name_keys)
    WHERE case_name_keys IS NOT NULL;

-- 0252_docket_keys_index_fastupdate_off.sql
ALTER INDEX idx_decisions_docket_keys SET (fastupdate = off);

SELECT gin_clean_pending_list('idx_decisions_docket_keys');

-- 0253_decision_sources_rank_cnil_fix.sql
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
            WHEN 'ariane-dce'   THEN 45
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-capp'    THEN 30
            ELSE 0
        END
    ) STORED;

-- 0254_facet_value_matiere_parent_uid.sql
UPDATE facet_value SET parent_uid = 'matiere:CIVILE' WHERE uid IN (
    'matiere:BAUX',
    'matiere:CIVI',
    'matiere:CONSTRUCTION',
    'matiere:COPROPRIETE',
    'matiere:EXPROPRIATION',
    'matiere:FAMILLE',
    'matiere:NATIONALITE',
    'matiere:SURENDETTEMENT',
    'matiere:TERRES'
);

UPDATE facet_value SET parent_uid = 'matiere:SOCIALE' WHERE uid IN (
    'matiere:PROTECTION_SOCIALE',
    'matiere:PRUD_HOMALE'
);

UPDATE facet_value SET parent_uid = 'matiere:COMMERCIALE'
WHERE uid = 'matiere:PROCEDURES_COLLECTIVES';

UPDATE facet_value SET parent_uid = 'matiere:CRIMINELLE' WHERE uid IN (
    'matiere:CORRECTIONNELLE',
    'matiere:INSTRUCTION',
    'matiere:MINEURS'
);

-- 0255_idx_decisions_notable.sql
CREATE INDEX idx_decisions_notable ON decisions (id)
WHERE publication_codes && ARRAY['r', 'A', 'b', 'l', 'c', 'B', 'C+', 'R']::text[];

-- 0256_decisions_case_name_source.sql
ALTER TABLE decisions
    ADD COLUMN case_name_source text
        CHECK (case_name_source IN ('canon', 'ce-list', 'source', 'renvoi'));

CREATE FUNCTION lj_case_name_rank(src text) RETURNS int
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE src
        WHEN 'canon' THEN 3
        WHEN 'ce-list' THEN 2
        WHEN 'source' THEN 1
        WHEN 'renvoi' THEN 0
        ELSE -1
    END
$$;

-- 0257_idx_decisions_case_name_source.sql
CREATE INDEX decisions_case_name_source_idx ON decisions (case_name_source)
    WHERE case_name_source IS NOT NULL;

-- 0258_chamber_add_speciality_uid.sql
ALTER TABLE chamber ADD COLUMN speciality_uid text;

WITH tot AS (
    SELECT jurisdiction_code AS jc, chamber_key AS ck, count(*) AS n
    FROM decisions
    WHERE deleted_at IS NULL
      AND chamber_key IS NOT NULL AND jurisdiction_code IS NOT NULL
    GROUP BY 1, 2
), par AS (
    SELECT jurisdiction_code AS jc, chamber_key AS ck, matiere_uid AS m, count(*) AS n
    FROM decisions
    WHERE deleted_at IS NULL
      AND chamber_key IS NOT NULL AND jurisdiction_code IS NOT NULL
      AND matiere_uid IS NOT NULL
    GROUP BY 1, 2, 3
), qualifiee AS (
    SELECT t.jc, t.ck, min(p.m) AS m
    FROM tot t
    JOIN par p ON p.jc = t.jc AND p.ck = t.ck
    WHERE t.ck NOT LIKE 'MAT:%'
    GROUP BY t.jc, t.ck, t.n
    HAVING count(*) = 1 AND max(p.n) * 2 >= t.n
)
UPDATE chamber c
SET speciality_uid = q.m
FROM qualifiee q
WHERE c.jurisdiction_code = q.jc AND c.chamber_key = q.ck;

-- 0259_lj_party_fold_function.sql
CREATE OR REPLACE FUNCTION lj_party_fold(x text) RETURNS text
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT AS $$
  SELECT regexp_replace(
           translate(lower(x),
                     'àáâãäåçèéêëìíîïñòóôõöøùúûüýÿ',
                     'aaaaaaceeeeiiiinoooooouuuuyy'),
           '[^a-z0-9]', '', 'g')
$$;

-- 0260_decision_party_representation_edge.sql
ALTER TABLE decision_party
  ADD COLUMN IF NOT EXISTS party_ord integer,
  ADD COLUMN IF NOT EXISTS sub_ord integer;

CREATE INDEX IF NOT EXISTS decision_party_party_ord_idx
  ON decision_party (decision_id, party_ord)
  WHERE party_ord IS NOT NULL;

CREATE INDEX IF NOT EXISTS decision_party_fold_morale_idx
  ON decision_party (lj_party_fold(resolve_key))
  WHERE quality = 'party' AND nature IS DISTINCT FROM 'physique';

-- 0261_decisions_bm25_v5_drop_matiere.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY decisions_bm25_v5 ON decisions
USING bm25 (
  id,
  full_text,
  search_title,
  ((jurisdiction_type)::pdb.literal),
  ((legal_instruments)::pdb.literal),
  ((legal_article_composite)::pdb.literal),
  ((publication_codes)::pdb.literal),
  ((jurisdiction_code)::pdb.literal),
  ((chamber_key)::pdb.literal),
  ((legal_domain_uid)::pdb.literal),
  ((solution_uid)::pdb.literal),
  ((office_uid)::pdb.literal),
  ((procedure_uid)::pdb.literal),
  date_lecture,
  deleted_at
) WITH (key_field=id, text_fields='{"full_text": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}, "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}}');

-- 0262_decisions_bm25_v5_swap.sql
DROP INDEX decisions_bm25;

ALTER INDEX decisions_bm25_v5 RENAME TO decisions_bm25;

-- 0263_drop_decisions_matiere_uid.sql
ALTER TABLE decisions DROP COLUMN matiere_uid;

-- 0264_comptes_desabonnements_table.sql
CREATE TABLE comptes.desabonnements (
    email     TEXT PRIMARY KEY,
    origine   TEXT NOT NULL,
    ajoute_le TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 0265_users_add_locale_columns.sql
ALTER TABLE users
    ADD COLUMN browser_locale  text,
    ADD COLUMN provider_locale text;

-- 0266_auth_email_tokens_purpose_login.sql
ALTER TABLE auth_email_tokens DROP CONSTRAINT auth_email_tokens_purpose_check;
ALTER TABLE auth_email_tokens ADD CONSTRAINT auth_email_tokens_purpose_check
    CHECK (purpose IN ('confirmation', 'recovery', 'connexion',
                       'email_change_ancienne', 'email_change_nouvelle'));

-- 0267_rename_desabonnements_to_unsubscribes.sql
ALTER TABLE comptes.desabonnements RENAME TO unsubscribes;
ALTER INDEX comptes.desabonnements_pkey RENAME TO unsubscribes_pkey;

-- 0268_facet_value_jurisdiction_type_sort.sql
UPDATE facet_value SET sort = 1  WHERE uid = 'jurisdiction_type:CC';
UPDATE facet_value SET sort = 2  WHERE uid = 'jurisdiction_type:CA';
UPDATE facet_value SET sort = 3  WHERE uid = 'jurisdiction_type:TJ';
UPDATE facet_value SET sort = 4  WHERE uid = 'jurisdiction_type:TCOM';
UPDATE facet_value SET sort = 5  WHERE uid = 'jurisdiction_type:CE';
UPDATE facet_value SET sort = 6  WHERE uid = 'jurisdiction_type:CAA';
UPDATE facet_value SET sort = 7  WHERE uid = 'jurisdiction_type:TA';
UPDATE facet_value SET sort = 8  WHERE uid = 'jurisdiction_type:CJUE';
UPDATE facet_value SET sort = 9  WHERE uid = 'jurisdiction_type:CEDH';
UPDATE facet_value SET sort = 10 WHERE uid = 'jurisdiction_type:CONSTIT';
UPDATE facet_value SET sort = 11 WHERE uid = 'jurisdiction_type:TC';
UPDATE facet_value SET sort = 12 WHERE uid = 'jurisdiction_type:CNDA';
UPDATE facet_value SET sort = 13 WHERE uid = 'jurisdiction_type:CNIL';

-- 0269_facet_value_publication_hints.sql
UPDATE facet_value SET hint = 'Retenue dans le rapport annuel de la Cour de cassation, la plus étroite de ses sélections. Code source r.'
WHERE uid = 'publication:PUBLIE_RAPPORT';

UPDATE facet_value SET hint = 'Retenue au Bulletin des arrêts, où la Cour de cassation publie chaque mois la jurisprudence qu''elle veut faire connaître. Code source b.'
WHERE uid = 'publication:PUBLIE_BULLETIN';

UPDATE facet_value SET hint = 'Signalée dans la lettre que sa chambre publie pour ses propres lecteurs, un palier sous le Bulletin. Code source l.'
WHERE uid = 'publication:LETTRE_CHAMBRE';

UPDATE facet_value SET hint = 'Accompagnée d''un communiqué de la Cour de cassation, qui en explique la portée. Code source c.'
WHERE uid = 'publication:COMMUNIQUE';

UPDATE facet_value SET hint = 'Rendue par la Cour de cassation sans être retenue au Bulletin. Code source n.'
WHERE uid = 'publication:INEDIT_BULLETIN';

UPDATE facet_value SET hint = 'Retenue au recueil Lebon, la publication de référence de la jurisprudence administrative. Code source A.'
WHERE uid = 'publication:PUBLIE_LEBON';

UPDATE facet_value SET hint = 'Citée aux tables du recueil Lebon, un palier sous la publication au recueil. Couvre les codes source B, C+ et R.'
WHERE uid = 'publication:MENTIONNE_LEBON';

UPDATE facet_value SET hint = 'Rendue par une juridiction administrative sans être retenue au recueil Lebon. Couvre les codes source C, D et Z.'
WHERE uid = 'publication:INEDIT_LEBON';

-- 0270_facet_value_matiere_labels.sql
UPDATE facet_value SET label = 'Chambre de la protection sociale'   WHERE uid = 'matiere:PROTECTION_SOCIALE';
UPDATE facet_value SET label = 'Chambre des procédures collectives' WHERE uid = 'matiere:PROCEDURES_COLLECTIVES';
UPDATE facet_value SET label = 'Chambre des étrangers'              WHERE uid = 'matiere:ETRANGERS';
UPDATE facet_value SET label = 'Chambre de l''expropriation'        WHERE uid = 'matiere:EXPROPRIATION';
UPDATE facet_value SET label = 'Chambre de proximité'               WHERE uid = 'matiere:PROXIMITE';
UPDATE facet_value SET label = 'Chambre du surendettement'          WHERE uid = 'matiere:SURENDETTEMENT';
UPDATE facet_value SET label = 'Chambre de la copropriété'          WHERE uid = 'matiere:COPROPRIETE';
UPDATE facet_value SET label = 'Chambre des urgences'               WHERE uid = 'matiere:URGENCES';
UPDATE facet_value SET label = 'Chambre du droit au logement (DALO)' WHERE uid = 'matiere:DALO';
UPDATE facet_value SET label = 'Chambre de la nationalité'          WHERE uid = 'matiere:NATIONALITE';
UPDATE facet_value SET label = 'Commission d''indemnisation des victimes (CIVI)' WHERE uid = 'matiere:CIVI';

-- 0271_legal_text_base_piece_uid.sql
ALTER TABLE legal_text ADD COLUMN base_piece_uid TEXT;

-- 0272_profil_acheteur_echecs_consecutifs.sql
ALTER TABLE profil_acheteur
  ADD COLUMN echecs_consecutifs integer NOT NULL DEFAULT 0;

-- 0273_chamber_label_nullable_truncate.sql
ALTER TABLE chamber ALTER COLUMN label DROP NOT NULL;
TRUNCATE TABLE chamber;

-- 0274_chamber_label_default_empty.sql
ALTER TABLE chamber ALTER COLUMN label SET DEFAULT '';

-- 0275_corpus_counts_rename_and_seed.sql
INSERT INTO decision_facet_counts (dimension, value, parent, n)
    SELECT 'legal_text_total'::text, ''::text, NULL::text, count(*) FROM legal_text
UNION ALL
    SELECT 'legal_article_total', '', NULL, count(*) FROM (
        SELECT a.text_uid, a.num_key FROM legal_article a
         WHERE a.date_debut <= CURRENT_DATE
           AND (a.date_fin IS NULL OR a.date_fin >= CURRENT_DATE)
           AND a.kind = 'article'
         GROUP BY 1, 2) ids;

ALTER TABLE decision_facet_counts RENAME TO corpus_counts;
ALTER TABLE corpus_counts RENAME CONSTRAINT decision_facet_counts_key TO corpus_counts_key;
CREATE VIEW decision_facet_counts AS SELECT * FROM corpus_counts;

-- 0276_chamber_drop_label.sql
ALTER TABLE chamber DROP COLUMN label;

-- 0277_api_keys.sql
CREATE TABLE api_keys (
    key_sha256   bytea PRIMARY KEY,
    user_sub     text NOT NULL REFERENCES users(sub) ON DELETE CASCADE,
    label        text NOT NULL,
    prefix       text NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz,
    revoked_at   timestamptz
);

CREATE INDEX api_keys_user_sub_idx ON api_keys (user_sub) WHERE revoked_at IS NULL;

-- 0278_drop_decision_facet_counts_view.sql
DROP VIEW decision_facet_counts;

-- 0279_drop_unsubscribes.sql
DROP TABLE comptes.unsubscribes;

-- 0280_chamber_drop_speciality_uid.sql
ALTER TABLE chamber DROP COLUMN IF EXISTS speciality_uid;

-- 0281_facet_value_chambre_du_conseil.sql
INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('procedure:CHAMBRE_DU_CONSEIL', 'procedure', 'Chambre du conseil',   14),
    ('matiere:REFERES',              'matiere',   'Chambre des référés',  25),
    ('matiere:PRESSE',               'matiere',   'Chambre de la presse', 26);

DELETE FROM facet_value WHERE uid = 'matiere:CONSEIL';

-- 0282_decisions_seat_and_seat_paths.sql
ALTER TABLE decisions
  ADD COLUMN seat       jsonb,
  ADD COLUMN seat_paths text[];

-- 0283_quantum_preuve_equations_verdict.sql
ALTER TABLE quantum_observation
  ADD COLUMN preuve    text,
  ADD COLUMN equations smallint;

ALTER TABLE decision_quantum
  ADD COLUMN verdict jsonb;

-- 0284_quantum_observation_one_row_per_decision.sql
ALTER TABLE quantum_observation
    DROP CONSTRAINT quantum_observation_decision_id_pc_extract_version_fkey,
    DROP CONSTRAINT quantum_observation_pkey,
    ADD PRIMARY KEY (decision_id, victime_ord, poste_ord);

DROP INDEX idx_quantum_observation_poste;
CREATE INDEX idx_quantum_observation_poste ON quantum_observation (poste, axe);

ALTER TABLE decision_quantum
    DROP CONSTRAINT decision_quantum_pkey,
    ADD PRIMARY KEY (decision_id),
    DROP COLUMN verdict;

ALTER TABLE quantum_observation
    ADD FOREIGN KEY (decision_id) REFERENCES decision_quantum (decision_id) ON DELETE CASCADE;

-- 0285_quantum_observation_origine_indemnites.sql
ALTER TABLE quantum_observation
  ADD COLUMN origine                   text,
  ADD COLUMN cite                      text,
  ADD COLUMN indemnites_acquises_cents bigint;

-- 0286_decisions_bm25_v6_seat_paths.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY decisions_bm25_v6 ON decisions
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
  date_lecture,
  deleted_at
) WITH (key_field=id, text_fields='{"full_text": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}, "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}}');

-- 0287_decisions_seat_paths_gin_index.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY idx_decisions_seat_paths ON decisions
USING gin (seat_paths) WITH (fastupdate=off) WHERE (deleted_at IS NULL);

-- 0288_user_search_history_chamber_to_seat.sql
WITH mapping(old_value, path) AS (VALUES
    ('cc|SOCIALE',      'CC.SOCIALE'),
    ('cc|COMMERCIALE',  'CC.COMMERCIALE'),
    ('cc|CRIMINELLE',   'CC.CRIMINELLE'),
    ('cc|C2.CIVILE',    'CC.C2_CIVILE'),
    ('cc|C3.CIVILE',    'CC.C3_CIVILE'),
    ('ca_paris|P5.C4',  'CA.PARIS.P5.C4'),
    ('ca_paris|P5.C5',  'CA.PARIS.P5.C5'),
    ('ca_paris|P5.C9',  'CA.PARIS.P5.C9'),
    ('ca_paris|P5.C11', 'CA.PARIS.P5.C11')
), target AS (
    SELECT h.id,
           array_agg(DISTINCT m.path) FILTER (WHERE m.path IS NOT NULL) AS paths,
           array_agg(DISTINCT v.value) FILTER (WHERE m.path IS NULL) AS kept
    FROM comptes.user_search_history h
         CROSS JOIN LATERAL jsonb_array_elements_text(h.filters -> 'chamber') AS v(value)
         LEFT JOIN mapping m ON m.old_value = v.value
    WHERE h.filters ? 'chamber'
    GROUP BY h.id
)
UPDATE comptes.user_search_history h
SET filters = CASE
                  WHEN target.kept IS NULL THEN h.filters - 'chamber'
                  ELSE jsonb_set(h.filters, '{chamber}', to_jsonb(target.kept))
              END
              || jsonb_build_object('seat', to_jsonb(target.paths))
FROM target
WHERE h.id = target.id AND target.paths IS NOT NULL;

-- 0289_decisions_bm25_swap_seat_paths.sql
DROP INDEX decisions_bm25;

ALTER INDEX decisions_bm25_v6 RENAME TO decisions_bm25;

