-- Thèmes Judilibre (ADR 0159) : liste verbatim `source_fields->'themes'`
-- (matière → chaîne de mots-clés) matérialisée en colonne, remplie par la
-- re-extraction v13. Vocabulaire ouvert (~7 400 matières distinctes, casse et
-- nomenclatures hétérogènes) → texte verbatim, PAS une facette référentielle.
ALTER TABLE decisions
    ADD COLUMN themes text[] NOT NULL DEFAULT '{}';
