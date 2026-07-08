-- Migration 0006 — Rendre gzip_blob nullable avant suppression
-- gzip_blob / raw_bytes / gzip_bytes sont redondants avec source_xml_gzip
-- depuis que l'API parse le XML à la volée. DROP NOT NULL = changement
-- de catalogue uniquement (pas de réécriture de table, verrou bref).
-- Le DROP COLUMN final est dans 0007, à jouer après l'arrêt de l'ingest.

ALTER TABLE decision_full_text ALTER COLUMN gzip_blob  DROP NOT NULL;
ALTER TABLE decision_full_text ALTER COLUMN raw_bytes  DROP NOT NULL;
ALTER TABLE decision_full_text ALTER COLUMN gzip_bytes DROP NOT NULL;
