-- Moteur interrogé par une recherche (ADR 0251) : `decisions` | `textes`.
-- Les recherches de normes (page /textes, outil MCP search_legal_articles)
-- n'étaient pas enregistrées — aucune trace en base des parcours croisés
-- décisions ↔ textes. Axe orthogonal à `source` (web | mcp).
ALTER TABLE user_search_history
    ADD COLUMN engine TEXT NOT NULL DEFAULT 'decisions';
