-- Migration 0007 — Supprimer gzip_blob, raw_bytes, gzip_bytes
-- À jouer APRÈS l'arrêt complet de tout process ingest utilisant l'ancien code.
-- Libère ~1.7 GB. VACUUM FULL decision_full_text ensuite pour récupérer l'espace disque.

ALTER TABLE decision_full_text DROP COLUMN IF EXISTS gzip_blob;
ALTER TABLE decision_full_text DROP COLUMN IF EXISTS raw_bytes;
ALTER TABLE decision_full_text DROP COLUMN IF EXISTS gzip_bytes;
