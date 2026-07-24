-- 0124 — legal_text.upcoming_versions : dates de versions futures du texte
-- (META_TEXTE_CHRONICLE/VERSIONS_A_VENIR du fond LEGI, ADR 0178). Posée par le
-- setter dédié set_legal_text_upcoming_versions (un seul écrivain : l'ingest
-- LEGI) ; la sentinelle 2222-02-22 (date inconnue) est conservée telle quelle.

ALTER TABLE legal_text
    ADD COLUMN upcoming_versions date[] NOT NULL DEFAULT '{}';
