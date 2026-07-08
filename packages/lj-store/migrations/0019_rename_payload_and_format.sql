-- Migration 0019 — Renomme decision_full_text.xml_gzip → source_payload_gzip
-- et ajoute payload_format ('xml'|'json') pour distinguer XML opendata et JSON
-- Judilibre. Cf. ADR 0031.

ALTER TABLE decision_full_text RENAME COLUMN xml_gzip TO source_payload_gzip;

ALTER TABLE decision_full_text
  ADD COLUMN payload_format TEXT NOT NULL DEFAULT 'xml'
    CHECK (payload_format IN ('xml', 'json'));

-- DROP DEFAULT après le backfill : les inserts futurs doivent déclarer le
-- format explicitement, on ne veut pas qu'un nouveau payload Judilibre soit
-- mal taggé par défaut.
ALTER TABLE decision_full_text ALTER COLUMN payload_format DROP DEFAULT;
