-- Migration 0129 — CNIL comme juridiction de décision (ADR 0185).
--
-- Nouveau `juridiction_type = CNIL` (délibérations/décisions de la CNIL, fond
-- bulk DILA `dila-cnil`). Deux effets :
--   1. Label du référentiel (`facet_value` namespace `juridiction:*`, complète le
--      seed 0102) — servi par l'API depuis le cache référentiel.
--   2. Barème d'autorité `decision_sources.source_rank` : `dila-cnil` aligné sur
--      les autres fonds DILA (50). Colonne GÉNÉRÉE → DROP + ADD (réécriture
--      ACCESS EXCLUSIVE, comme migration 0070 dont ceci est le port + `dila-cnil`).
--
-- La table `jurisdiction` n'est PAS seedée ici (comme 0100/0102) : la ligne CNIL
-- naît à l'ingest via `ensure_jurisdictions` (data-driven depuis `jurisdiction_ref`).

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
