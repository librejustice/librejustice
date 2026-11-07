-- 0167_decisions_jur_code_date_partial_idx.sql
DROP INDEX idx_decisions_jur_code_date;
CREATE INDEX idx_decisions_jur_code_date ON decisions (jurisdiction_code, date_lecture DESC, id)
  WHERE deleted_at IS NULL AND public_id IS NOT NULL;

-- 0168_legal_text_curated_flag.sql
ALTER TABLE legal_text ADD COLUMN curated boolean NOT NULL DEFAULT false;

UPDATE legal_text t
SET curated = true
WHERE t.text_uid NOT LIKE 'BOI-%'
  AND EXISTS (
    SELECT 1 FROM legal_article a
    WHERE a.text_uid = t.text_uid
      AND a.source_uid LIKE t.text_uid || '#%');

-- 0169_legal_article_usage_text_uid_idx.sql
DROP INDEX legal_article_usage_bm25;
CREATE INDEX legal_article_usage_bm25 ON legal_article_usage USING bm25 (
  id, terms, ((text_uid)::pdb.literal)
) WITH (key_field=id, text_fields='{
    "terms": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}_-]+", "ascii_folding": true}, "record": "freq"}
  }');

-- 0170_entity_folded_gin_fastupdate_off.sql
ALTER INDEX entity_folded_all_idx SET (fastupdate = off);
SELECT gin_clean_pending_list('entity_folded_all_idx');

-- 0171_legal_citation_terms_gin_fastupdate_off.sql
ALTER INDEX legal_citation_terms_idx SET (fastupdate = off);
SELECT gin_clean_pending_list('legal_citation_terms_idx');

-- 0172_decision_chunks_gin_fastupdate_off.sql
ALTER INDEX idx_chunks_legal_article_composite SET (fastupdate = off);
SELECT gin_clean_pending_list('idx_chunks_legal_article_composite');

ALTER INDEX idx_chunks_legal_instruments SET (fastupdate = off);
SELECT gin_clean_pending_list('idx_chunks_legal_instruments');

-- 0173_legal_citation_autovacuum_tuning.sql
ALTER TABLE legal_citation SET (
  autovacuum_vacuum_scale_factor = 0.02,
  autovacuum_analyze_scale_factor = 0.05
);

-- 0174_article_commentary_slug_anchor.sql
DELETE FROM article_commentaire
WHERE text_uid NOT IN (SELECT text_uid FROM legal_text WHERE slug IS NOT NULL);

ALTER TABLE article_commentaire ADD COLUMN IF NOT EXISTS text_slug TEXT;

UPDATE article_commentaire ac
SET text_slug = lt.slug
FROM legal_text lt
WHERE lt.text_uid = ac.text_uid AND ac.text_slug IS NULL;

ALTER TABLE article_commentaire ALTER COLUMN text_slug SET NOT NULL;

ALTER TABLE article_commentaire ALTER COLUMN text_uid DROP NOT NULL;

CREATE INDEX IF NOT EXISTS article_commentaire_slug_anchor_idx
    ON article_commentaire (text_slug, num_key)
    WHERE deleted_at IS NULL;

-- 0175_article_commentary_drop_text_uid.sql
DROP INDEX IF EXISTS article_commentaire_anchor_idx;
ALTER TABLE article_commentaire DROP COLUMN IF EXISTS text_uid;

-- 0176_entity_uid_pattern_idx.sql
CREATE INDEX IF NOT EXISTS entity_uid_pattern_idx ON entity (uid text_pattern_ops);

-- 0177_legal_article_reading_rank.sql
ALTER TABLE legal_article ADD COLUMN reading_rank integer;

CREATE INDEX IF NOT EXISTS idx_legal_article_reading_rank
  ON legal_article (text_uid, reading_rank);

CREATE INDEX IF NOT EXISTS idx_legal_article_reading_rank_stale
  ON legal_article (text_uid) WHERE reading_rank IS NULL;

-- 0178_entity_activity_affiliation.sql
ALTER TABLE entity ADD COLUMN activity jsonb;

CREATE FUNCTION lj_active_at(activity jsonb, d date) RETURNS boolean
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT activity IS NULL OR d IS NULL OR COALESCE(
        (SELECT bool_or(
                (NOT s ? 'du' OR (s->>'du')::date <= d)
            AND (NOT s ? 'au' OR (s->>'au')::date >= d))
         FROM jsonb_array_elements(activity) s),
        true)
$$;

CREATE TABLE entity_affiliation (
    entity_uid    text NOT NULL,
    structure_uid text NOT NULL,
    du            date,
    au            date,
    source        text NOT NULL
);

CREATE UNIQUE INDEX entity_affiliation_open_idx
    ON entity_affiliation (entity_uid, source) WHERE au IS NULL;
CREATE INDEX entity_affiliation_entity_idx ON entity_affiliation (entity_uid);
CREATE INDEX entity_affiliation_structure_idx ON entity_affiliation (structure_uid);

-- 0179_public_procurement_notices.sql
CREATE TABLE annonce (
    uid              text PRIMARY KEY,
    fond             text NOT NULL,
    famille_uid      text REFERENCES facet_value(uid),
    titre            text NOT NULL,

    acheteur_libelle text NOT NULL,
    departement      text,
    cpv_uid          text REFERENCES facet_value(uid),
    date_publication date NOT NULL,
    echeance         timestamptz,
    closed_at        timestamptz,
    url_action       text NOT NULL,
    details          jsonb NOT NULL

);

CREATE INDEX annonce_ouverte_echeance_idx ON annonce (fond, echeance)
    WHERE closed_at IS NULL;

CREATE TABLE annonce_entity (
    annonce_uid text NOT NULL REFERENCES annonce(uid),
    entity_uid  text NOT NULL,
    role        text NOT NULL,
    PRIMARY KEY (annonce_uid, entity_uid, role)
);

CREATE INDEX annonce_entity_entity_idx ON annonce_entity (entity_uid);

CREATE TABLE annonce_source (
    source_uid       text PRIMARY KEY,
    annonce_uid      text NOT NULL REFERENCES annonce(uid),
    url_avis         text,
    first_seen_at    timestamptz NOT NULL,
    last_seen_at     timestamptz NOT NULL,
    content_checksum bigint NOT NULL
);
CREATE INDEX annonce_source_annonce_idx ON annonce_source (annonce_uid);

CREATE TABLE profil_acheteur (
    host          text PRIMARY KEY,
    moteur        text,
    avis_boamp    int NOT NULL DEFAULT 0,
    first_seen_at timestamptz NOT NULL,
    last_seen_at  timestamptz NOT NULL
);

INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('nature_marche:TRAVAUX',     'nature_marche', 'Travaux',     1),
    ('nature_marche:SERVICES',    'nature_marche', 'Services',    2),
    ('nature_marche:FOURNITURES', 'nature_marche', 'Fournitures', 3),
    ('nature_marche:AUTRES',      'nature_marche', 'Autres',      4);

INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('cpv:03', 'cpv', 'Produits agricoles, de l''élevage, de la pêche, de la sylviculture', 3),
    ('cpv:09', 'cpv', 'Produits pétroliers, combustibles, électricité et autres sources d''énergie', 9),
    ('cpv:14', 'cpv', 'Produits des industries extractives, métaux de base et produits connexes', 14),
    ('cpv:15', 'cpv', 'Produits alimentaires, boissons, tabac et produits connexes', 15),
    ('cpv:16', 'cpv', 'Machines agricoles', 16),
    ('cpv:18', 'cpv', 'Vêtements, articles chaussants, bagages et accessoires', 18),
    ('cpv:19', 'cpv', 'Cuir et textiles, matériaux en plastique et en caoutchouc', 19),
    ('cpv:22', 'cpv', 'Imprimés et produits connexes', 22),
    ('cpv:24', 'cpv', 'Produits chimiques', 24),
    ('cpv:30', 'cpv', 'Machines, matériel et fournitures informatiques et de bureau', 30),
    ('cpv:31', 'cpv', 'Machines, appareils, équipements et consommables électriques ; éclairage', 31),
    ('cpv:32', 'cpv', 'Équipements de radio, de télévision, de communication et de télécommunication', 32),
    ('cpv:33', 'cpv', 'Matériels médicaux, produits pharmaceutiques et produits de soins personnels', 33),
    ('cpv:34', 'cpv', 'Équipement de transport et produits auxiliaires pour le transport', 34),
    ('cpv:35', 'cpv', 'Équipement de sécurité, de lutte contre l''incendie, de police et de défense', 35),
    ('cpv:37', 'cpv', 'Instruments de musique, articles de sport, jeux, jouets, artisanat et arts', 37),
    ('cpv:38', 'cpv', 'Équipements de laboratoire, d''optique et de précision', 38),
    ('cpv:39', 'cpv', 'Meubles, aménagements, appareils électroménagers et produits de nettoyage', 39),
    ('cpv:41', 'cpv', 'Eau collectée et purifiée', 41),
    ('cpv:42', 'cpv', 'Machines industrielles', 42),
    ('cpv:43', 'cpv', 'Machines pour l''industrie minière, l''exploitation de carrières et la construction', 43),
    ('cpv:44', 'cpv', 'Structures et matériaux de construction ; produits auxiliaires', 44),
    ('cpv:45', 'cpv', 'Travaux de construction', 45),
    ('cpv:48', 'cpv', 'Logiciels et systèmes d''information', 48),
    ('cpv:50', 'cpv', 'Services de réparation et d''entretien', 50),
    ('cpv:51', 'cpv', 'Services d''installation', 51),
    ('cpv:55', 'cpv', 'Services d''hôtellerie, de restauration et de commerce au détail', 55),
    ('cpv:60', 'cpv', 'Services de transport', 60),
    ('cpv:63', 'cpv', 'Services d''appui et services auxiliaires des transports ; agences de voyages', 63),
    ('cpv:64', 'cpv', 'Services des postes et télécommunications', 64),
    ('cpv:65', 'cpv', 'Services publics', 65),
    ('cpv:66', 'cpv', 'Services financiers et d''assurance', 66),
    ('cpv:70', 'cpv', 'Services immobiliers', 70),
    ('cpv:71', 'cpv', 'Services d''architecture, de construction, d''ingénierie et d''inspection', 71),
    ('cpv:72', 'cpv', 'Services de technologies de l''information', 72),
    ('cpv:73', 'cpv', 'Services de recherche et développement et services de conseil connexes', 73),
    ('cpv:75', 'cpv', 'Services de l''administration publique, de la défense et de la sécurité sociale', 75),
    ('cpv:76', 'cpv', 'Services relatifs à l''industrie du pétrole et du gaz', 76),
    ('cpv:77', 'cpv', 'Services agricoles, sylvicoles, horticoles, d''aquaculture et d''apiculture', 77),
    ('cpv:79', 'cpv', 'Services aux entreprises : droit, marketing, conseil, recrutement, impression', 79),
    ('cpv:80', 'cpv', 'Services d''enseignement et de formation', 80),
    ('cpv:85', 'cpv', 'Services de santé et services sociaux', 85),
    ('cpv:90', 'cpv', 'Services d''assainissement, de déchets, d''hygiénisation et d''environnement', 90),
    ('cpv:92', 'cpv', 'Services récréatifs, culturels et sportifs', 92),
    ('cpv:98', 'cpv', 'Autres services communautaires, sociaux et personnels', 98);

ANALYZE facet_value;

-- 0180_legal_citation_family_by_date.sql
CREATE OR REPLACE FUNCTION public._suivants_family_keys(
    p_uid text, p_num_key text, p_date date
) RETURNS text[]
LANGUAGE sql
STABLE
AS $function$
    WITH anchor AS (
        SELECT owner_uid, min(seq) AS seq
        FROM legal_toc_edge
        WHERE text_uid = p_uid AND child_kind = 'article'
          AND child_num_key = p_num_key
          AND (date_debut IS NULL OR date_debut <= p_date)
          AND (date_fin   IS NULL OR date_fin   >  p_date)
        GROUP BY owner_uid
    ),
    members AS (
        SELECT a.owner_uid, e.child_num_key, min(e.seq) AS seq_min
        FROM anchor a
        JOIN legal_toc_edge e
          ON e.owner_uid = a.owner_uid
         AND e.child_kind = 'article'
         AND e.seq >= a.seq
         AND (e.date_debut IS NULL OR e.date_debut <= p_date)
         AND (e.date_fin   IS NULL OR e.date_fin   >  p_date)
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
$function$;

CREATE OR REPLACE FUNCTION public._sync_decisions_legal_instruments_for(p_ids bigint[])
RETURNS bigint
LANGUAGE sql
AS $function$
    WITH pairs AS (
        SELECT lc.decision_id, el->>2 AS ref_text_uid,
               (el->>2) || '|' || k AS composite
        FROM legal_citation lc
        JOIN decisions d0 ON d0.id = lc.decision_id
        CROSS JOIN LATERAL jsonb_array_elements(lc.spans) AS el
        LEFT JOIN LATERAL unnest(
            CASE WHEN (el->>4)::bool AND el->>3 IS NOT NULL
                 THEN coalesce(_suivants_family_keys(el->>2, el->>3,
                                   coalesce(d0.date_lecture, CURRENT_DATE)),
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
$function$;

CREATE OR REPLACE FUNCTION public._sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS bigint
LANGUAGE sql
AS $function$
    WITH pairs AS (
        SELECT lc.decision_id, el->>2 AS ref_text_uid,
               (el->>2) || '|' || k AS composite
        FROM legal_citation lc
        JOIN decisions d0 ON d0.id = lc.decision_id
        CROSS JOIN LATERAL jsonb_array_elements(lc.spans) AS el
        LEFT JOIN LATERAL unnest(
            CASE WHEN (el->>4)::bool AND el->>3 IS NOT NULL
                 THEN coalesce(_suivants_family_keys(el->>2, el->>3,
                                   coalesce(d0.date_lecture, CURRENT_DATE)),
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
$function$;

DROP FUNCTION public._suivants_family_keys(text, text);

-- 0181_decisions_updated_at_content_date.sql
UPDATE decisions
SET updated_at = date_lecture::timestamptz
WHERE date_lecture IS NOT NULL
  AND updated_at::date <> date_lecture;

-- 0182_decision_party_firm_ord.sql
ALTER TABLE decision_party ADD COLUMN firm_ord int;

CREATE INDEX decision_party_firm_ord_idx ON decision_party (decision_id, firm_ord)
    WHERE firm_ord IS NOT NULL;

-- 0183_drop_legal_text_curated.sql
ALTER TABLE legal_text DROP COLUMN curated;

-- 0184_procurement_notice_stream_idx.sql
DROP INDEX annonce_ouverte_echeance_idx;

CREATE INDEX annonce_fond_echeance_idx ON annonce (fond, echeance);

-- 0185_legal_text_bm25_v2_facet_fields.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY legal_text_body_bm25_v2 ON legal_text
USING bm25 (
    id,
    title,
    body,
    (nature::pdb.literal),
    (jurisdiction::pdb.literal),
    (slug::pdb.literal),
    (role::pdb.literal),
    ((coalesce(status, 'VIGUEUR'))::pdb.literal('alias=status_eff')),
    ((upper(nature))::pdb.literal('alias=nature_up')),
    ((lower(nature))::pdb.literal('alias=source')),
    ((body IS NOT NULL)::pdb.alias('has_body'))
)
WITH (
  key_field = 'id',
  text_fields = '{
    "title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "body":  {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}
  }'
);

-- 0186_legal_article_kind.sql
ALTER TABLE legal_article
    ADD COLUMN kind text NOT NULL DEFAULT 'article';

COMMENT ON COLUMN legal_article.kind IS
  'Nature de la ligne : ''article'' (norme ou publication) | ''edit'' (dispositif consommé, LEGI n''en garde que la mention de sa cible). Posée par refresh_article_denorm depuis le corps.';

-- 0187_legal_article_bm25_v2_validity_window.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY legal_article_bm25_v2 ON legal_article
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
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "texte":        {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "num":          {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "basic"}
  }'
);

-- 0188_legal_text_bm25_index_swap.sql
DROP INDEX legal_text_body_bm25;

ALTER INDEX legal_text_body_bm25_v2 RENAME TO legal_text_body_bm25;

-- 0189_legal_article_bm25_index_swap.sql
DROP INDEX legal_article_bm25;

ALTER INDEX legal_article_bm25_v2 RENAME TO legal_article_bm25;

-- 0190_decision_sources_drop_id.sql
ALTER TABLE decision_sources DROP COLUMN id;

-- 0191_notice_rename_actor_drop_stale.sql
ALTER TABLE annonce RENAME COLUMN acheteur_libelle TO acteur_libelle;
ALTER TABLE annonce DROP COLUMN closed_at;
ALTER TABLE annonce_source DROP COLUMN last_seen_at;

-- 0192_legal_text_bm25_v2_text_uid.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY legal_text_body_bm25_v2 ON legal_text
USING bm25 (
    id,
    title,
    body,
    (text_uid::pdb.literal),
    (nature::pdb.literal),
    (jurisdiction::pdb.literal),
    (slug::pdb.literal),
    (role::pdb.literal),
    ((coalesce(status, 'VIGUEUR'))::pdb.literal('alias=status_eff')),
    ((upper(nature))::pdb.literal('alias=nature_up')),
    ((lower(nature))::pdb.literal('alias=source')),
    ((body IS NOT NULL)::pdb.alias('has_body'))
)
WITH (
  key_field = 'id',
  text_fields = '{
    "title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "body":  {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}
  }'
);

-- 0193_legal_text_bm25_text_uid_swap.sql
DROP INDEX legal_text_body_bm25;

ALTER INDEX legal_text_body_bm25_v2 RENAME TO legal_text_body_bm25;

-- 0194_notice_source_drop_checksum.sql
ALTER TABLE annonce_source DROP COLUMN content_checksum;

-- 0195_mcp_tokens_sha256_hashing.sql
ALTER TABLE mcp_tokens ADD COLUMN access_token_sha256 BYTEA;
UPDATE mcp_tokens SET access_token_sha256 = sha256(access_token::bytea);
ALTER TABLE mcp_tokens ALTER COLUMN access_token_sha256 SET NOT NULL;

ALTER TABLE mcp_tokens DROP CONSTRAINT mcp_tokens_pkey;
ALTER TABLE mcp_tokens ADD PRIMARY KEY (access_token_sha256);
ALTER TABLE mcp_tokens ALTER COLUMN access_token DROP NOT NULL;

ALTER TABLE mcp_refresh_tokens ADD COLUMN refresh_token_sha256 BYTEA;
UPDATE mcp_refresh_tokens SET refresh_token_sha256 = sha256(refresh_token::bytea);
ALTER TABLE mcp_refresh_tokens ALTER COLUMN refresh_token_sha256 SET NOT NULL;

ALTER TABLE mcp_refresh_tokens DROP CONSTRAINT mcp_refresh_tokens_pkey;
ALTER TABLE mcp_refresh_tokens ADD PRIMARY KEY (refresh_token_sha256);
ALTER TABLE mcp_refresh_tokens ALTER COLUMN refresh_token DROP NOT NULL;

CREATE FUNCTION mcp_token_derive_sha256() RETURNS trigger AS $$
BEGIN
    IF TG_TABLE_NAME = 'mcp_tokens' THEN
        NEW.access_token_sha256 := COALESCE(
            NEW.access_token_sha256, sha256(NEW.access_token::bytea)
        );
    ELSE
        NEW.refresh_token_sha256 := COALESCE(
            NEW.refresh_token_sha256, sha256(NEW.refresh_token::bytea)
        );
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER mcp_tokens_sha256
    BEFORE INSERT ON mcp_tokens
    FOR EACH ROW EXECUTE FUNCTION mcp_token_derive_sha256();

CREATE TRIGGER mcp_refresh_tokens_sha256
    BEFORE INSERT ON mcp_refresh_tokens
    FOR EACH ROW EXECUTE FUNCTION mcp_token_derive_sha256();

-- 0196_users_sessions_valid_from.sql
ALTER TABLE users ADD COLUMN sessions_valid_from TIMESTAMPTZ;

COMMENT ON COLUMN users.sessions_valid_from IS
    'Tout JWT Supabase dont iat est antérieur est refusé. NULL = jamais révoqué. '
    'Posé par la déconnexion globale ; il n''existe pas de table de sessions.';

-- 0197_notice_title_actor_trgm_idx.sql
CREATE INDEX IF NOT EXISTS annonce_titre_trgm_idx
    ON annonce USING gin (titre gin_trgm_ops);

CREATE INDEX IF NOT EXISTS annonce_acteur_libelle_trgm_idx
    ON annonce USING gin (acteur_libelle gin_trgm_ops);

-- 0198_ingest_cursor.sql
CREATE TABLE IF NOT EXISTS ingest_cursor (
    job        text PRIMARY KEY,
    position   text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);

-- 0199_drop_ingest_cursor.sql
DROP TABLE IF EXISTS ingest_cursor;

-- 0200_drop_decision_sources_active_idx.sql
DROP INDEX idx_decision_sources_active;

-- 0201_notice_redirect.sql
CREATE TABLE annonce_redirect (
    ancien_uid text PRIMARY KEY,
    nouvel_uid text NOT NULL REFERENCES annonce(uid),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX annonce_redirect_nouvel_idx ON annonce_redirect (nouvel_uid);

-- 0202_mcp_tokens_drop_plaintext.sql
DROP TRIGGER mcp_tokens_sha256 ON mcp_tokens;
DROP TRIGGER mcp_refresh_tokens_sha256 ON mcp_refresh_tokens;
DROP FUNCTION mcp_token_derive_sha256();

ALTER TABLE mcp_tokens DROP COLUMN access_token;
ALTER TABLE mcp_refresh_tokens DROP COLUMN refresh_token;

-- 0203_article_scope_and_geo_facets.sql
CREATE TABLE article_scope (
    text_uid   text NOT NULL,
    num_key    text NOT NULL,
    date_debut date NOT NULL,
    mode       text NOT NULL CHECK (mode IN ('etend', 'ecarte', 'restreint')),
    scope      text NOT NULL REFERENCES facet_value(uid),
    s_debut    date NOT NULL,
    s_fin      date,
    source     text,
    PRIMARY KEY (text_uid, num_key, date_debut, mode, scope, s_debut),
    CHECK (s_fin IS NULL OR s_fin > s_debut),
    CHECK (split_part(scope, ':', 1) IN ('geo', 'natio', 'statut', 'communaute'))
);

COMMENT ON TABLE article_scope IS
  'Événements d''applicabilité d''une version d''article. Absence '
  'de ligne = domaine par défaut de la juridiction du texte. « Applicable à '
  'D dans S » : aucun ecarte couvrant (D, S ou ancêtre) ET (un etend/restreint '
  'couvre (D, S ou ancêtre) OU (pas de restreint actif ET D dans '
  '[date_debut, date_fin) ET S dans le domaine par défaut)).';

INSERT INTO facet_value (uid, facet, label, abbr, parent_uid, sort) VALUES
    ('geo:FR',     'geo', 'France (ordre juridique)',  NULL,   NULL,     0),
    ('geo:FR-M',   'geo', 'France métropolitaine',     NULL,   'geo:FR', 1),
    ('geo:FR-ALS', 'geo', 'Alsace-Moselle',            NULL,   'geo:FR-M', 2),
    ('geo:GP',     'geo', 'Guadeloupe',                NULL,   'geo:FR', 10),
    ('geo:MQ',     'geo', 'Martinique',                NULL,   'geo:FR', 11),
    ('geo:GF',     'geo', 'Guyane',                    NULL,   'geo:FR', 12),
    ('geo:RE',     'geo', 'La Réunion',                NULL,   'geo:FR', 13),
    ('geo:YT',     'geo', 'Mayotte',                   NULL,   'geo:FR', 14),
    ('geo:PM',     'geo', 'Saint-Pierre-et-Miquelon',  'SPM',  'geo:FR', 20),
    ('geo:BL',     'geo', 'Saint-Barthélemy',          NULL,   'geo:FR', 21),
    ('geo:MF',     'geo', 'Saint-Martin',              NULL,   'geo:FR', 22),
    ('geo:NC',     'geo', 'Nouvelle-Calédonie',        NULL,   'geo:FR', 30),
    ('geo:PF',     'geo', 'Polynésie française',       NULL,   'geo:FR', 31),
    ('geo:WF',     'geo', 'Wallis-et-Futuna',          NULL,   'geo:FR', 32),
    ('geo:TF',     'geo', 'Terres australes',          'TAAF', 'geo:FR', 33);

-- 0204_text_filiation.sql
CREATE TABLE text_filiation (
    child_text_uid  text NOT NULL REFERENCES legal_text(text_uid) ON DELETE CASCADE,
    parent_text_uid text NOT NULL REFERENCES legal_text(text_uid) ON DELETE CASCADE,
    fork_date       date,
    num_compatible  boolean NOT NULL,
    reception_act   text,
    note            text,
    PRIMARY KEY (child_text_uid, parent_text_uid),
    CHECK (child_text_uid <> parent_text_uid)
);

CREATE INDEX text_filiation_parent_idx ON text_filiation (parent_text_uid);

INSERT INTO text_filiation (child_text_uid, parent_text_uid, fork_date, num_compatible, reception_act, note)
SELECT v.child_text_uid, v.parent_text_uid, v.fork_date::date, v.num_compatible, v.reception_act, v.note
FROM (VALUES
    ('BE/CODE-CIVIL', 'LEGITEXT000006070721', '1815-06-09', true,
     'Loi du 9 vendémiaire an IV (annexion) ; code du 21 mars 1804',
     'Divergence à la fin de la souveraineté française (acte final du Congrès de Vienne).'),
    ('LU/CODE-CIVIL', 'LEGITEXT000006070721', '1815-06-09', true,
     'Loi du 9 vendémiaire an IV (département des Forêts)',
     'Divergence à la création du Grand-Duché (Congrès de Vienne).'),
    ('HT/CODE-CIVIL', 'LEGITEXT000006070721', '1826-05-01', false,
     'Code civil haïtien de 1825, art. 2047 (en vigueur le 1er mai 1826)',
     'Calqué sur l''état 1804 pré-Restauration, RENUMÉROTÉ (FR 1382 = HT 1168) : fusion de timeline interdite sans concordance.'),
    ('MU/CODE-CIVIL', 'LEGITEXT000006070721', '1810-12-03', true,
     'Décret de Decaen du 21 avril 1808 (Code Napoléon)',
     'Gel à la capitulation britannique ; regreffes mauriciennes ultérieures sur du droit français du XXe siècle.'),
    ('CM/CODE-CIVIL', 'LEGITEXT000006070721', '1960-01-01', true,
     'Décret du 22 mai 1924 (extension de la législation AEF)',
     'Gel à l''indépendance ; promulgations locales tracées dans le texte (B.A.S. 1856…).'),
    ('TD/CODE-CIVIL', 'LEGITEXT000006070721', '1959-03-31', true,
     'Acte législatif n° 1 portant constitution du 31 mars 1959 ; loi constitutionnelle n° 2/62 du 16 avril 1962',
     'Code français de 1958 gelé ; greffes locales datées article par article (J.O. A.E.F. 1913…).'),
    ('TG/CODE-CIVIL', 'LEGITEXT000006070721', '1956-05-01', true,
     'Continuité : loi organique du 23 avril 1960, art. 31',
     'Édition « à jour au 1er mai 1956 » ; branche la mieux conservée (1 001+ articles identiques).')
) AS v (child_text_uid, parent_text_uid, fork_date, num_compatible, reception_act, note)
WHERE EXISTS (SELECT 1 FROM legal_text WHERE text_uid = v.child_text_uid)
  AND EXISTS (SELECT 1 FROM legal_text WHERE text_uid = v.parent_text_uid);

-- 0205_scope_membership_default_scope.sql
CREATE TABLE scope_membership (
    member_scope    text NOT NULL REFERENCES facet_value(uid),
    community_scope text NOT NULL REFERENCES facet_value(uid),
    m_debut         date NOT NULL,
    m_fin           date,
    source          text,
    PRIMARY KEY (member_scope, community_scope, m_debut),
    CHECK (m_fin IS NULL OR m_fin > m_debut),
    CHECK (split_part(community_scope, ':', 1) = 'communaute')
);

ALTER TABLE legal_text ADD COLUMN default_scope text REFERENCES facet_value(uid);

INSERT INTO facet_value (uid, facet, label, abbr, parent_uid, sort) VALUES
    ('communaute:OHADA', 'communaute', 'OHADA (droit uniforme des affaires)', 'OHADA', NULL, 0),
    ('communaute:UEMOA', 'communaute', 'Union économique et monétaire ouest-africaine', 'UEMOA', NULL, 1),
    ('communaute:CIMA',  'communaute', 'Conférence interafricaine des marchés d''assurances', 'CIMA', NULL, 2),
    ('communaute:CEMAC', 'communaute', 'Communauté économique et monétaire de l''Afrique centrale', 'CEMAC', NULL, 3);

INSERT INTO facet_value (uid, facet, label, abbr, parent_uid, sort) VALUES
    ('geo:AM', 'geo', 'Arménie',                 NULL, NULL, 100),
    ('geo:AO', 'geo', 'Angola',                  NULL, NULL, 101),
    ('geo:AT', 'geo', 'Autriche',                NULL, NULL, 102),
    ('geo:BE', 'geo', 'Belgique',                NULL, NULL, 103),
    ('geo:BF', 'geo', 'Burkina Faso',            NULL, NULL, 104),
    ('geo:BG', 'geo', 'Bulgarie',                NULL, NULL, 105),
    ('geo:BI', 'geo', 'Burundi',                 NULL, NULL, 106),
    ('geo:BJ', 'geo', 'Bénin',                   NULL, NULL, 107),
    ('geo:CD', 'geo', 'République démocratique du Congo', 'RDC', NULL, 108),
    ('geo:CF', 'geo', 'République centrafricaine', 'RCA', NULL, 109),
    ('geo:CG', 'geo', 'Congo',                   NULL, NULL, 110),
    ('geo:CH', 'geo', 'Suisse',                  NULL, NULL, 111),
    ('geo:CI', 'geo', 'Côte d''Ivoire',          NULL, NULL, 112),
    ('geo:CM', 'geo', 'Cameroun',                NULL, NULL, 113),
    ('geo:DE', 'geo', 'Allemagne',               NULL, NULL, 114),
    ('geo:DJ', 'geo', 'Djibouti',                NULL, NULL, 115),
    ('geo:DO', 'geo', 'République dominicaine',  NULL, NULL, 116),
    ('geo:DZ', 'geo', 'Algérie',                 NULL, NULL, 117),
    ('geo:EG', 'geo', 'Égypte',                  NULL, NULL, 118),
    ('geo:ES', 'geo', 'Espagne',                 NULL, NULL, 119),
    ('geo:GA', 'geo', 'Gabon',                   NULL, NULL, 120),
    ('geo:GN', 'geo', 'Guinée',                  NULL, NULL, 121),
    ('geo:GR', 'geo', 'Grèce',                   NULL, NULL, 122),
    ('geo:HT', 'geo', 'Haïti',                   NULL, NULL, 123),
    ('geo:HU', 'geo', 'Hongrie',                 NULL, NULL, 124),
    ('geo:IQ', 'geo', 'Irak',                    NULL, NULL, 125),
    ('geo:IT', 'geo', 'Italie',                  NULL, NULL, 126),
    ('geo:JO', 'geo', 'Jordanie',                NULL, NULL, 127),
    ('geo:KM', 'geo', 'Comores',                 NULL, NULL, 128),
    ('geo:LB', 'geo', 'Liban',                   NULL, NULL, 129),
    ('geo:LU', 'geo', 'Luxembourg',              NULL, NULL, 130),
    ('geo:MA', 'geo', 'Maroc',                   NULL, NULL, 131),
    ('geo:MC', 'geo', 'Monaco',                  NULL, NULL, 132),
    ('geo:MG', 'geo', 'Madagascar',              NULL, NULL, 133),
    ('geo:ML', 'geo', 'Mali',                    NULL, NULL, 134),
    ('geo:MR', 'geo', 'Mauritanie',              NULL, NULL, 135),
    ('geo:MU', 'geo', 'Maurice',                 NULL, NULL, 136),
    ('geo:NE', 'geo', 'Niger',                   NULL, NULL, 137),
    ('geo:NG', 'geo', 'Nigéria',                 NULL, NULL, 138),
    ('geo:NL', 'geo', 'Pays-Bas',                NULL, NULL, 139),
    ('geo:PE', 'geo', 'Pérou',                   NULL, NULL, 140),
    ('geo:PL', 'geo', 'Pologne',                 NULL, NULL, 141),
    ('geo:PT', 'geo', 'Portugal',                NULL, NULL, 142),
    ('geo:RO', 'geo', 'Roumanie',                NULL, NULL, 143),
    ('geo:RS', 'geo', 'Serbie',                  NULL, NULL, 144),
    ('geo:RU', 'geo', 'Russie',                  NULL, NULL, 145),
    ('geo:RW', 'geo', 'Rwanda',                  NULL, NULL, 146),
    ('geo:SN', 'geo', 'Sénégal',                 NULL, NULL, 147),
    ('geo:ST', 'geo', 'Sao Tomé-et-Principe',    NULL, NULL, 148),
    ('geo:SY', 'geo', 'Syrie',                   NULL, NULL, 149),
    ('geo:TD', 'geo', 'Tchad',                   NULL, NULL, 150),
    ('geo:TG', 'geo', 'Togo',                    NULL, NULL, 151),
    ('geo:TN', 'geo', 'Tunisie',                 NULL, NULL, 152),
    ('geo:TR', 'geo', 'Turquie',                 NULL, NULL, 153),
    ('geo:UA', 'geo', 'Ukraine',                 NULL, NULL, 154),
    ('geo:VE', 'geo', 'Venezuela',               NULL, NULL, 155),
    ('geo:VN', 'geo', 'Viêt Nam',                NULL, NULL, 156);

INSERT INTO scope_membership (member_scope, community_scope, m_debut, m_fin, source) VALUES
    ('geo:SN', 'communaute:OHADA', '1995-09-18', NULL,
     'Traité de Port-Louis du 17 octobre 1993, entré en vigueur le 18 septembre 1995 ; Sénégal ratifié le 14 juin 1994'),
    ('geo:CD', 'communaute:OHADA', '2012-09-12', NULL,
     'Adhésion de la RDC, entrée en vigueur le 12 septembre 2012');

UPDATE legal_text SET default_scope = 'geo:FR-ALS'
WHERE text_uid IN ('JORFTEXT000000869866', 'JORFTEXT000000869867');
UPDATE legal_text SET default_scope = 'geo:PF'
WHERE text_uid IN ('JORFTEXT000024961711', 'JORFTEXT000024961743');

-- 0206_legal_article_usage_bm25_v2_num_key.sql
-- lj:sans-transaction
CREATE INDEX CONCURRENTLY legal_article_usage_bm25_v2 ON legal_article_usage
USING bm25 (
  id,
  terms,
  ((text_uid)::pdb.literal),
  ((num_key)::pdb.literal)
) WITH (key_field=id, text_fields='{
    "terms": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}_-]+", "ascii_folding": true}, "record": "freq"}
  }');

-- 0207_legal_article_usage_bm25_swap.sql
DROP INDEX legal_article_usage_bm25;

ALTER INDEX legal_article_usage_bm25_v2 RENAME TO legal_article_usage_bm25;

