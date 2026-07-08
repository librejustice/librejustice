-- ECLI n'est PAS unique (ADR 0107, révise ADR 0080).
--
-- Judilibre réutilise un même ECLI sur des décisions distinctes (vieux arrêts :
-- ex. ECLI:FR:CCASS:1960:SO560 sur 3 arrêts de dates différentes — 128 ECLI
-- partagés / 498 décisions au constat 2026-06-16). L'index UNIQUE
-- `idx_decisions_ecli` (0056) reposait sur une hypothèse d'unicité fausse : il
-- faisait échouer le backfill ECLI et n'aurait pu être maintenu. On le remplace
-- par un index partiel **non unique** (toujours utile pour le lookup
-- `find_decision_by_ecli`). La dédup ECLI-first ne fusionne plus que sur un ECLI
-- **non ambigu** (exactement une décision active) — repli sur `canonical_ref`
-- sinon (cf. `find_decision_by_ecli`).
DROP INDEX IF EXISTS idx_decisions_ecli;

CREATE INDEX IF NOT EXISTS idx_decisions_ecli
ON decisions (ecli)
WHERE ecli IS NOT NULL;
