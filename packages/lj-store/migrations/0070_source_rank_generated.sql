-- `decision_sources.source_rank` devient une colonne GÉNÉRÉE depuis `source`
-- (ADR 0113, révise ADR 0098 §3).
--
-- Le rang d'autorité (gagnant = max) est une fonction pure de `source` : le
-- stocker en colonne libre écrite à l'ingest a dérivé (1 033 490 provenances
-- opendata figées à 40 — leur rang au moment de l'ingest — alors que le barème
-- code valait 55, faisant gagner jade(50) sur l'overlap admin contre l'intention
-- ADR 0098). Une colonne `GENERATED ALWAYS … STORED` recalcule le rang depuis
-- `source` à chaque écriture : barème unique dans la DDL, drift structurellement
-- impossible, `ORDER BY source_rank DESC` inchangé (les index/tris la lisent
-- comme une colonne normale).
--
-- DROP puis ADD : on ne peut pas convertir une colonne libre en générée
-- in-place. ADD … STORED réécrit la table (ACCESS EXCLUSIVE) — coût accepté.
-- Barème = port exact de l'ancienne `support::source_rank` (supprimée).
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
