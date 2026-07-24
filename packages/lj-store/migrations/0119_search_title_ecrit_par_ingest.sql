-- Migration 0119 — `decisions.search_title` écrit par l'ingest (ADR 0170 ét.5).
--
-- La colonne était GENERATED ALWAYS … STORED sur `formation_or_chamber`
-- (migration 0015), condamnée par le drop de cette colonne (séquencement
-- 0170). DROP EXPRESSION la convertit en colonne simple en CONSERVANT les
-- valeurs courantes (les index BM25 qui la portent restent servis) ; l'ingest
-- la compose désormais via `lj_core::titles` (juridiction référentielle,
-- siège recomposé depuis les axes, date FR, premier numéro) et le backfill
-- `reextract-fields --full --overwrite` réécrit tout le fonds.
ALTER TABLE decisions ALTER COLUMN search_title DROP EXPRESSION;
