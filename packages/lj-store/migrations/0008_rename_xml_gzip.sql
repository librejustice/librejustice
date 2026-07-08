-- Migration 0008 — Renommer source_xml_gzip → xml_gzip + drop colonnes obsolètes
-- À jouer APRÈS l'arrêt complet de tout process ingest utilisant l'ancien code.
-- (0007 doit être jouée avant ou dans la même transaction)

ALTER TABLE decision_full_text DROP COLUMN IF EXISTS gzip_blob;
ALTER TABLE decision_full_text DROP COLUMN IF EXISTS raw_bytes;
ALTER TABLE decision_full_text DROP COLUMN IF EXISTS gzip_bytes;
ALTER TABLE decision_full_text RENAME COLUMN source_xml_gzip TO xml_gzip;

-- Récupère ~1.7 GB : lancer ensuite VACUUM FULL decision_full_text;
