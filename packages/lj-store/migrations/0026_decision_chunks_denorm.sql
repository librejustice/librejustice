-- Migration 0026 — Pre-filter BM25 via dénormalisation des métadonnées sur
-- ``decision_chunks`` (cf. ADR 0033).
--
-- Ce que cette migration fait :
--
-- 1. Supprime ``decisions.deleted_at`` (jamais filtrant : ``mark_deleted``
--    purge déjà chunks + payload, donc un chunk orphelin n'existe pas).
-- 2. Crée la table ``decision_legal_references(decision_id, instrument,
--    article)`` et y migre le contenu de ``decisions.legal_references``
--    (JSONB), puis drop la colonne JSONB.
-- 3. Dénormalise sur ``decision_chunks`` les colonnes filtrables côté search,
--    en droppant d'abord ``chunks_vec`` + ``chunks_bm25`` (réindexés en
--    section 6 sur la heap finale).
-- 4. Backfill et triggers de propagation.
-- 5. Rebuild ``chunks_bm25`` (fast fields) + ``chunks_vec`` (IVF VectorChord
--    ``vchordrq`` + RaBitQ 8-bit) en fin de migration.
--
-- Optimisation cruciale : on drop ``chunks_vec`` + ``chunks_bm25`` AVANT le
-- backfill des chunks. Sans cela, chaque UPDATE sur N chunks coûte une
-- insertion IVF (recherche du centroïde + append dans la posting list) plus
-- une réécriture BM25 — pour finalement les jeter au rebuild. Premier essai
-- (1 M chunks) : 2h13 ; ce pattern descend à ~50 min.
--
-- ⚠️ Post-migration obligatoire : ``REINDEX INDEX CONCURRENTLY`` sur
-- ``chunks_bm25`` ET ``chunks_vec``. La migration laisse les segments
-- Tantivy bloated (jusqu'à 2× la taille cible) car le bgmerger ParadeDB ne
-- compacte pas spontanément les segments > 1 GB, et ``vchordrq`` accumule
-- des pages réservées-mais-vides. Sur 1 M chunks mesuré : 14 GB bloat
-- → 4.2 GB après REINDEX BM25, 4 GB → 1.5 GB après REINDEX vec. Cf.
-- commande ``librejustice db reindex-search``.

-- Cap maintenance memory pour accélérer sorts, hash-joins et builds d'index.
-- SET LOCAL : portée transaction (le migrator wrappe tout en une tx).
SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

-- =====================================================================
-- 1. Drop de decisions.deleted_at
-- =====================================================================

-- Hard-delete des décisions actuellement soft-deleted (chunks et payload
-- déjà purgés par mark_deleted ; cf. repository.py).
DELETE FROM decisions WHERE deleted_at IS NOT NULL;

-- Les partial indexes ``WHERE deleted_at IS NULL`` deviennent inutiles ;
-- on les recrée sans la condition.
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

-- =====================================================================
-- 2. Refonte legal_references JSONB → table relationnelle
-- =====================================================================

CREATE TABLE decision_legal_references (
    decision_id BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    instrument  TEXT   NOT NULL,
    article     TEXT
);

CREATE INDEX idx_dlr_decision   ON decision_legal_references (decision_id);
CREATE INDEX idx_dlr_article    ON decision_legal_references (article text_pattern_ops)
  WHERE article IS NOT NULL;
-- Hash plutôt que B-tree : on n'a besoin que de l'égalité sur instrument
-- (filtres ``WHERE instrument = X``, agrégats facet par instrument), pas de
-- range / LIKE. Le hash ignore la limite B-tree de 2704 octets — robuste si
-- un futur ingest pousse un blob long avant que l'extraction ne soit fixée.
CREATE INDEX idx_dlr_instrument ON decision_legal_references USING hash (instrument);

-- Backfill depuis decisions.legal_references JSONB.
-- Forme attendue : [{"instrument": "...", "articles": ["...", ...]}, ...]
-- Si "articles" est vide ou absent, on insère une ligne (instrument, NULL).
--
-- Outliers d'extraction Gemini : ~842 refs (0.03 %) ont concaténé un blabla
-- procédural d'audience après le nom valide d'instrument (typique :
-- "Code de justice administrative. Les parties ont été régulièrement
-- averties…"  → 24 KB pour le pire cas). Les ``articles`` parsés
-- restent propres (e.g. ``['R. 611-7', 'R. 611-7-3', 'L. 911-1']``), donc on
-- récupère le préfixe valide via ``split_part(., '. ', 1)`` plutôt que de
-- dropper la ligne entière.
--
-- Heuristique : appliquer le split uniquement si length > 200 (préserve
-- les références courtes du type "Code électoral, notamment son article
-- L. 51" qui contiennent un ". " légitime). Filet final : length BETWEEN
-- 1 AND 500 pour shunter les ~8 rows résiduelles encore patho après split
-- (règlements UE concaténés, etc.) et éviter tout overflow B-tree futur.
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

-- =====================================================================
-- 3. Dénormalisation des métadonnées sur decision_chunks
-- =====================================================================

-- Drop des index lourds AVANT le backfill — cf. commentaire en tête.
-- chunks_vec (IVF VectorChord ``vchordrq``, RaBitQ 8-bit sur 1024-dim) et
-- chunks_bm25 (Tantivy ParadeDB) sont reconstruits en section 6 sur la
-- heap finale.
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

-- Backfill unique combinant scalaires (depuis ``decisions``) et agrégat
-- ``legal_instruments`` (depuis ``decision_legal_references``). Une seule
-- passe sur la heap → moitié moins de tuple-versions, moitié moins de bloat.
-- LEFT JOIN LATERAL : si une décision n'a aucune référence légale, l'agrégat
-- retourne NULL et la colonne reste NULL (pas de filtre WHERE perdu).
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

-- Indexes pre-filter (B-tree pour les scalaires, GIN pour le tableau).
CREATE INDEX idx_chunks_jurisdiction_level ON decision_chunks (jurisdiction_level);
CREATE INDEX idx_chunks_instance_level     ON decision_chunks (instance_level);
CREATE INDEX idx_chunks_main_outcome       ON decision_chunks (main_outcome);
CREATE INDEX idx_chunks_special_procedure  ON decision_chunks (special_procedure);
CREATE INDEX idx_chunks_jurisdiction_name  ON decision_chunks (LOWER(jurisdiction_name));
CREATE INDEX idx_chunks_is_recueil         ON decision_chunks (is_recueil);
CREATE INDEX idx_chunks_is_tables_lebon    ON decision_chunks (is_tables_lebon);
CREATE INDEX idx_chunks_date_lecture       ON decision_chunks (date_lecture);
CREATE INDEX idx_chunks_legal_instruments  ON decision_chunks USING GIN (legal_instruments);

-- =====================================================================
-- 4. Triggers de propagation decisions → decision_chunks
-- =====================================================================

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

-- =====================================================================
-- 5. Trigger de propagation decision_legal_references → decision_chunks
-- =====================================================================

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

-- =====================================================================
-- 6. Rebuild chunks_bm25 (fast fields) + chunks_vec (IVF VectorChord)
-- =====================================================================
--
-- Les deux index ont été droppés en début de section 3. On les rebuild ici
-- une fois la heap stabilisée — passage cache-friendly, vs. N insertions
-- coûteuses pendant le backfill.
--
-- chunks_bm25 — l'index initial (cf. 0001 / 0020) ne couvrait que ``id``,
-- ``body``, ``juridiction_type`` et faisait un post-filter heap-side après
-- le scan Tantivy pour les autres filtres. On intègre maintenant toutes les
-- colonnes filtrables comme fast fields :
--
-- - colonnes catégorielles (enum-like, valeurs déjà normalisées) →
--   ``pdb.literal`` : un token unique, casse préservée, columnar par défaut
--   → ParadeDB peut pushdown ``c.col = ANY(...)`` directement dans le scan.
-- - ``jurisdiction_name`` → ``pdb.literal_normalized`` : lowercase implicite
--   pour matcher le filtre ``LOWER(c.jurisdiction_name)`` côté search.py
--   sans dépendre de la casse de la valeur source.
-- - ``legal_instruments TEXT[]`` → ``pdb.literal`` : chaque élément du
--   tableau devient un token, ParadeDB pushdown ``&&`` / ``= ANY``.
-- - ``date_lecture`` (date) + ``is_recueil`` / ``is_tables_lebon`` (bool) :
--   colonnes brutes — ParadeDB stocke columnar par défaut pour ces types.
-- - ``body`` : regex tokenizer + position recording (config strictement
--   identique à 0020 — la phrase scoring S5 en dépend).
--
-- chunks_vec — paramètres identiques à 0023 (``rabitq8_cosine_ops``).
--
-- ⚠️ Build long. Pendant le rebuild :
-- - writes sur ``decision_chunks`` bloquées (CREATE INDEX prend un SHARE
--   lock — pas concurrent, sinon il faudrait sortir du transaction wrapper
--   du migrator).
-- - jambe BM25 search : l'opérateur ``@@@`` requiert l'index, donc erreur
--   ou seq scan. Jambe ANN : idem (VectorChord requis pour ``<#>``).
-- - À lancer en maintenance off-peak.

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
