-- Migration 0032 — Suppression de ``decisions.content_hash`` (sha256).
--
-- La transition sha256 → xxh3-64 a été initiée par la migration 0018
-- (ajout de ``content_checksum`` nullable). Le backfill rétroactif depuis
-- ``decision_full_text.source_payload_gzip`` ayant été effectué (sha256
-- recalculé et comparé pour validation, 0 mismatch sur 689 937 rows),
-- toutes les lignes ont désormais un ``content_checksum`` non-NULL.
--
-- On peut donc :
--   1. Marquer ``content_checksum`` NOT NULL (contrainte d'intégrité).
--   2. Supprimer ``content_hash`` — plus aucun code ne s'en sert.

ALTER TABLE decisions ALTER COLUMN content_checksum SET NOT NULL;
ALTER TABLE decisions DROP COLUMN content_hash;
