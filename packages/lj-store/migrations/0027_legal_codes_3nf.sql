-- Migration 0027 — Refonte 3NF de decision_legal_references (cf. ADR 0034).
--
-- Ce que cette migration fait :
--
-- 1. Crée ``legal_codes`` et ``legal_articles`` (référentiels canon).
-- 2. Backfille depuis ``decision_legal_references`` brut (zéro normalisation
--    des noms — préservation 100% pour ne pas toucher à
--    ``decision_chunks.legal_instruments``).
-- 3. Crée ``decision_legal_references_v2(decision_id, article_id)`` avec
--    PK absorbant les ~34 doublons exacts résiduels.
-- 4. DROP CASCADE de l'ancienne table (drop son trigger) + RENAME de v2.
-- 5. Recréation du trigger ``dlr_sync_chunks_legal_instruments`` sur le
--    nouveau shape (JOIN à 3 tables, retourne le même ARRAY_AGG TEXT[]).
--
-- ``decision_chunks.legal_instruments`` n'est PAS recalculé :
-- - La trigger v2 produirait exactement les mêmes valeurs (mêmes
--   ``legal_codes.name``).
-- - Elle est créée APRÈS l'INSERT initial dans v2, donc ne fire pas.
-- - Conséquence : aucun chunk touché, aucun rebuild ``chunks_bm25`` /
--   ``chunks_vec`` requis. Migration online (modulo ~1s d'ACCESS EXCLUSIVE
--   sur ``decision_legal_references`` au DROP+RENAME).

SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

-- =====================================================================
-- 1. Nouvelles tables référentielles
-- =====================================================================

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

-- =====================================================================
-- 2. Backfill legal_codes (raw : préservation des chaînes existantes)
-- =====================================================================

INSERT INTO legal_codes (name)
SELECT DISTINCT instrument
FROM decision_legal_references;

-- =====================================================================
-- 3. Backfill legal_articles
-- =====================================================================

INSERT INTO legal_articles (code_id, label)
SELECT DISTINCT lc.id, dlr.article
FROM decision_legal_references dlr
JOIN legal_codes lc ON lc.name = dlr.instrument;

-- =====================================================================
-- 4. Table de liaison v2
-- =====================================================================

CREATE TABLE decision_legal_references_v2 (
    decision_id BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    article_id  BIGINT NOT NULL REFERENCES legal_articles(id),
    PRIMARY KEY (decision_id, article_id)
);

-- ``DISTINCT`` absorbe les doublons exacts (~34 rows mesurés) ; la PK
-- les rejetterait sinon.
--
-- ``la.label = dlr.article`` (pas ``IS NOT DISTINCT FROM``) car les données
-- actuelles n'ont aucune ligne avec ``article IS NULL`` (audit pré-migration :
-- 0 sur 5.54M). ``=`` permet un hash join — divise le temps par ~10x vs
-- ``IS NOT DISTINCT FROM`` qui force un nested loop. Le schéma autorise
-- toujours NULL côté ``legal_articles.label`` pour les futurs inserts via
-- ``replace_legal_references`` (un instrument cité sans articles).
INSERT INTO decision_legal_references_v2 (decision_id, article_id)
SELECT DISTINCT dlr.decision_id, la.id
FROM decision_legal_references dlr
JOIN legal_codes    lc ON lc.name    = dlr.instrument
JOIN legal_articles la ON la.code_id = lc.id
                       AND la.label  = dlr.article;

-- =====================================================================
-- 5. Swap atomique
-- =====================================================================
--
-- ``DROP CASCADE`` retire aussi le trigger ``dlr_sync_chunks_legal_instruments``
-- qui était attaché à l'ancienne table. La fonction
-- ``sync_chunks_legal_instruments()`` survit (objet schéma indépendant), elle
-- sera redéfinie en section 6 pour le nouveau shape.

DROP TABLE decision_legal_references CASCADE;
ALTER TABLE decision_legal_references_v2 RENAME TO decision_legal_references;
ALTER INDEX decision_legal_references_v2_pkey RENAME TO decision_legal_references_pkey;

-- Index sur ``article_id`` créé ici (après DROP) pour éviter la collision
-- de nom avec l'ancien ``idx_dlr_article`` de la migration 0026 (qui était
-- sur la colonne ``article TEXT``, devenue inutile en 3NF).
CREATE INDEX idx_dlr_article_id ON decision_legal_references(article_id);

-- =====================================================================
-- 6. Trigger v2 — JOIN sur le nouveau shape, même sortie ARRAY_AGG TEXT[]
-- =====================================================================

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
