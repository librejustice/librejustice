-- Migration 0042 : préférence « enregistrer mon activité » (mode ZDR)
-- ADR : docs/adr/0056-track-activity-zero-data-retention.md
-- Étend ADR 0036/0039 : l'utilisateur peut couper tout enregistrement
-- (recherches, lectures, signets). 'true' par défaut — comportement actuel.

ALTER TABLE users
    ADD COLUMN track_activity BOOLEAN NOT NULL DEFAULT true;
