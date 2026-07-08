-- Migration 0005 — Supprimer les métriques de taille inutiles du XML source

ALTER TABLE decision_full_text
DROP COLUMN IF EXISTS source_xml_raw_bytes;

ALTER TABLE decision_full_text
DROP COLUMN IF EXISTS source_xml_gzip_bytes;
