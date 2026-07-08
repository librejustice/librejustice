-- Migration 0074 — index trigram sur legal_text.title_key (ADR 0112 phase B, P5).
--
-- La résolution citation→texte EST la canonicalisation, ancrée sur le catalogue
-- (P5, cf. docs/working-notes) : une forme citée `text_key = normalize_instrument`
-- résout par match EXACT `title_key`, sinon par match FUZZY contre le catalogue.
-- Cet index trigram fournit les candidats fuzzy (le btree `idx_legal_text_title_key`
-- sert l'exact). Remplace le snap fréquentiel ADR 0079 : la cible canonique vient du
-- catalogue, pas de la fréquence du corpus.
--
-- Additif : ne change aucun comportement live (rien ne lit cet index tant que le
-- résolveur fuzzy n'est pas branché).

CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE INDEX idx_legal_text_title_key_trgm
    ON legal_text USING gin (title_key gin_trgm_ops);
