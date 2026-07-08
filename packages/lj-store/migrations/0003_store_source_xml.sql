-- Migration 0003 — Conserver aussi le XML source brut gzippé

ALTER TABLE decision_full_text
ADD COLUMN IF NOT EXISTS source_xml_gzip BYTEA;
